//! Experimental receiver-side activation of bounded cue pointers.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::PoisonError;

use codex_core::context::InternalContextSource;
use codex_core::context::InternalModelContextFragment;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ContextualUserFragment;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionEventSink;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::TurnInputContext;
use codex_extension_api::TurnInputContributor;
use codex_protocol::user_input::UserInput;
use serde::Deserialize;
use sha2::Digest;
use sha2::Sha256;

mod assessment;
use assessment::pointer;

const MAX_CATALOG: usize = 16 * 1024;

/// The model-visible effect permitted for cue activation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CueActivationMode {
    #[default]
    Off,
    Shadow,
    Advisory,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CueActivationConfig {
    pub mode: CueActivationMode,
    pub catalog_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema: String,
    cues: Vec<Cue>,
}

struct LoadedCatalog {
    catalog: Catalog,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cue {
    id: String,
    title: String,
    source: String,
    task_shape: Vec<String>,
    risk: String,
    discriminator: String,
    release_when: String,
    #[serde(default)]
    avoid_when: Vec<String>,
    #[serde(default)]
    provenance: HashMap<String, String>,
}

fn load(path: &Path) -> Result<LoadedCatalog, &'static str> {
    let file = std::fs::File::open(path).map_err(|_| "unreadable")?;
    let mut bytes = Vec::new();
    file.take((MAX_CATALOG + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "unreadable")?;
    if bytes.len() > MAX_CATALOG {
        return Err("oversized");
    }
    let catalog: Catalog = serde_json::from_slice(&bytes).map_err(|_| "malformed")?;
    validate(&catalog)?;
    Ok(LoadedCatalog {
        catalog,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    })
}

fn validate(catalog: &Catalog) -> Result<(), &'static str> {
    if catalog.schema != "codex.cue-header-catalog.v0" || catalog.cues.len() > 32 {
        return Err("invalid");
    }
    let mut ids = HashSet::new();
    for cue in &catalog.cues {
        let fields = [
            &cue.title,
            &cue.source,
            &cue.risk,
            &cue.discriminator,
            &cue.release_when,
        ];
        if !field(&cue.id, 64)
            || !ids.insert(&cue.id)
            || !fields.into_iter().all(|value| field(value, 512))
            || !handles(&cue.task_shape, true)
            || !handles(&cue.avoid_when, false)
            || cue.provenance.len() > 8
            || !cue
                .provenance
                .iter()
                .all(|(key, value)| field(key, 64) && field(value, 512))
        {
            return Err("invalid");
        }
    }
    Ok(())
}

fn field(value: &str, limit: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= limit
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '<' | '>'))
}

fn handles(values: &[String], required: bool) -> bool {
    (!required || !values.is_empty())
        && values.len() <= 8
        && values
            .iter()
            .all(|value| field(value, 64) && !norm(value).is_empty())
}

#[derive(Default)]
struct Query {
    segments: Vec<String>,
    cooling: HashSet<String>,
    represented: bool,
    use_prior: bool,
}

#[derive(Default)]
struct Runtime(Mutex<State>, OnceLock<()>);

#[derive(Default)]
struct State {
    index: u64,
    prior: VecDeque<String>,
    emitted_at: HashMap<String, u64>,
    pending_assessments: HashMap<String, assessment::PendingAssessment>,
}

impl Runtime {
    fn query(&self, inputs: &[UserInput]) -> Query {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.index = state.index.saturating_add(1);
        let current = request(inputs);
        let continuation = is_continuation(&current);
        let substantive = !continuation && !norm(&current).is_empty();
        let represented = !current.is_empty();
        let mut remaining = 4096usize.saturating_sub(current.len());
        let mut segments = vec![current.clone()];
        for prior in &state.prior {
            if norm(prior) != norm(&current) && remaining > 0 {
                let mut segment = String::new();
                append(&mut segment, prior, remaining);
                remaining -= segment.len();
                segments.push(segment);
            }
        }
        if substantive {
            state.prior.retain(|prior| norm(prior) != norm(&current));
            state.prior.push_front(current);
            state.prior.truncate(2);
        }
        let index = state.index;
        state
            .emitted_at
            .retain(|_, at| index.saturating_sub(*at) <= 2);
        let cooling = state.emitted_at.keys().cloned().collect();
        Query {
            segments,
            cooling,
            represented,
            use_prior: continuation,
        }
    }

    fn emitted(&self, id: &str) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let index = state.index;
        state.emitted_at.insert(id.to_string(), index);
    }
}

fn request(inputs: &[UserInput]) -> String {
    let mut output = String::new();
    for part in inputs.iter().filter_map(|input| match input {
        UserInput::Text { text, .. } => Some(text),
        UserInput::Skill { name, .. } | UserInput::Mention { name, .. } => Some(name),
        _ => None,
    }) {
        if !part.trim().is_empty() {
            append(&mut output, part, 2048);
        }
    }
    output
}

fn append(output: &mut String, value: &str, cap: usize) {
    if !output.is_empty() && output.len() < cap {
        output.push('\n');
    }
    let mut end = value.len().min(cap.saturating_sub(output.len()));
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&value[..end]);
}

fn norm(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_continuation(text: &str) -> bool {
    ["back", "back 3", "continue", "please continue", "proceed"].contains(&norm(text).as_str())
}

struct Selection<'a>(&'a Cue, &'a str, u16, usize);

fn select_candidate<'a>(
    catalog: &'a Catalog,
    query: &Query,
    include_cooling: bool,
) -> Option<Selection<'a>> {
    if !query.represented {
        return None;
    }
    // A continuation inherits the latest substantive scope, including an
    // exclusion or no-match. Older requests must not revive a superseded task.
    let segment_index = if query.use_prior && query.segments.len() > 1 {
        1
    } else {
        0
    };
    let segment = query.segments.get(segment_index)?;
    catalog
        .cues
        .iter()
        .filter(|cue| include_cooling || !query.cooling.contains(&cue.id))
        .filter_map(|cue| cue_score(cue, segment, segment_index))
        .filter(|selection| selection.2 > 0)
        .max_by(|left, right| {
            left.2
                .cmp(&right.2)
                .then_with(|| right.0.id.cmp(&left.0.id))
        })
}

fn cue_score<'a>(cue: &'a Cue, text: &str, segment_index: usize) -> Option<Selection<'a>> {
    let normalized = norm(text);
    let tokens = normalized.split_whitespace().collect::<HashSet<_>>();
    if cue
        .avoid_when
        .iter()
        .any(|handle| score(&normalized, &tokens, handle) > 0)
    {
        return None;
    }
    cue.task_shape
        .iter()
        .map(|handle| {
            Selection(
                cue,
                handle,
                score(&normalized, &tokens, handle),
                segment_index,
            )
        })
        .max_by_key(|selection| selection.2)
}

fn score(query: &str, tokens: &HashSet<&str>, handle: &str) -> u16 {
    let handle = norm(handle);
    let parts = handle.split_whitespace().collect::<Vec<_>>();
    if parts.is_empty() {
        0
    } else if format!(" {query} ").contains(&format!(" {handle} ")) {
        200 + parts.len() as u16
    } else if parts.iter().all(|part| tokens.contains(part)) {
        100 + parts.len() as u16
    } else {
        0
    }
}

struct Extension<C> {
    config: Arc<dyn Fn(&C) -> CueActivationConfig + Send + Sync>,
    event_sink: Arc<dyn ExtensionEventSink>,
}

impl<C: Send + Sync + 'static> ThreadLifecycleContributor<C> for Extension<C> {
    fn on_thread_start<'a>(&'a self, input: ThreadStartInput<'a, C>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            input.thread_store.insert((self.config)(input.config));
            input.thread_store.get_or_init(Runtime::default);
        })
    }
}

impl<C: Send + Sync + 'static> ConfigContributor<C> for Extension<C> {
    fn on_config_changed(&self, _: &ExtensionData, store: &ExtensionData, _: &C, next: &C) {
        store.insert((self.config)(next));
    }
}

impl<C: Send + Sync + 'static> TurnInputContributor for Extension<C> {
    fn contribute<'a>(
        &'a self,
        input: TurnInputContext<'a>,
        _: Option<Arc<dyn codex_extension_api::ExtensionMetrics>>,
        _: &'a ExtensionData,
        thread: &'a ExtensionData,
        turn: &'a ExtensionData,
    ) -> ExtensionFuture<'a, Vec<Box<dyn ContextualUserFragment + Send>>> {
        Box::pin(async move {
            let (Some(config), Some(runtime)) =
                (thread.get::<CueActivationConfig>(), thread.get::<Runtime>())
            else {
                return Vec::new();
            };
            if config.mode == CueActivationMode::Off {
                return Vec::new();
            }
            let Some(path) = config.catalog_path.as_deref() else {
                runtime.1.get_or_init(|| {
                    tracing::warn!(
                        event.name = "codex.cue_activation.decision",
                        thread_id = thread.level_id(),
                        turn_id = input.turn_id.as_str(),
                        status = "catalog_error",
                        reason = "unconfigured",
                        "cue activation decision"
                    )
                });
                return Vec::new();
            };
            let loaded = match load(path) {
                Ok(catalog) => catalog,
                Err(kind) => {
                    runtime.1.get_or_init(|| {
                        tracing::warn!(
                            event.name = "codex.cue_activation.decision",
                            thread_id = thread.level_id(),
                            turn_id = input.turn_id.as_str(),
                            status = "catalog_error",
                            reason = kind,
                            "cue activation decision"
                        )
                    });
                    return Vec::new();
                }
            };
            let query = runtime.query(&input.user_input);
            let selection = select_candidate(&loaded.catalog, &query, false);
            let Some(Selection(cue, handle, score, segment_index)) = selection else {
                if let Some(Selection(cue, _, _, segment_index)) =
                    select_candidate(&loaded.catalog, &query, true)
                {
                    let matched_scope = if segment_index == 0 {
                        "current".to_string()
                    } else {
                        format!("prior_{segment_index}")
                    };
                    tracing::debug!(
                        event.name = "codex.cue_activation.decision",
                        thread_id = thread.level_id(),
                        turn_id = input.turn_id.as_str(),
                        status = "cooldown",
                        mode = ?config.mode,
                        catalog_sha256 = loaded.sha256.as_str(),
                        cue_id = cue.id.as_str(),
                        matched_scope = matched_scope.as_str(),
                        "cue activation decision"
                    );
                    return Vec::new();
                }
                tracing::debug!(
                    event.name = "codex.cue_activation.decision",
                    thread_id = thread.level_id(),
                    turn_id = input.turn_id.as_str(),
                    status = if query.represented {
                        "no_match"
                    } else {
                        "unrepresented"
                    },
                    mode = ?config.mode,
                    catalog_sha256 = loaded.sha256.as_str(),
                    "cue activation decision"
                );
                return Vec::new();
            };
            let matched_scope = if segment_index == 0 {
                "current".to_string()
            } else {
                format!("prior_{segment_index}")
            };
            tracing::debug!(
                event.name = "codex.cue_activation.decision",
                thread_id = thread.level_id(),
                turn_id = input.turn_id.as_str(),
                status = "selected",
                mode = ?config.mode,
                catalog_sha256 = loaded.sha256.as_str(),
                cue_id = cue.id.as_str(),
                matched_handle = handle,
                matched_scope = matched_scope.as_str(),
                score,
                "cue activation decision"
            );
            runtime.emitted(&cue.id);
            if config.mode == CueActivationMode::Shadow {
                return Vec::new();
            }
            runtime.offer_assessment(&input.turn_id, &cue.id, &loaded.sha256);
            turn.insert(assessment::AssessmentOffer {
                turn_id: input.turn_id.clone(),
            });
            let fragment = InternalModelContextFragment::new(
                InternalContextSource::from_static("cue_activation"),
                pointer(cue, handle),
            );
            vec![Box::new(fragment) as Box<dyn ContextualUserFragment + Send>]
        })
    }
}

pub fn install<C>(
    registry: &mut ExtensionRegistryBuilder<C>,
    config: impl Fn(&C) -> CueActivationConfig + Send + Sync + 'static,
) where
    C: Send + Sync + 'static,
{
    let extension = Arc::new(Extension {
        config: Arc::new(config),
        event_sink: registry.event_sink(),
    });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.turn_input_contributor(extension.clone());
    registry.tool_contributor(extension.clone());
    registry.turn_lifecycle_contributor(extension);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "catalogue_tests.rs"]
mod catalogue_tests;

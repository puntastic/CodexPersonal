use super::*;
use pretty_assertions::assert_eq;

const FIXTURE: &str = include_str!("../tests/fixtures/synthetic-cues.json");
const POSITIVE: &str = "Please review this pull request and inspect behavior";
const INVALID: &str = "Please review the pull request description";
fn text(value: &str) -> Vec<UserInput> {
    vec![UserInput::Text {
        text: value.to_string(),
        text_elements: Vec::new(),
    }]
}
fn query(parts: &[&str]) -> Query {
    Query {
        segments: parts.iter().map(|part| (*part).into()).collect(),
        cooling: HashSet::new(),
        represented: true,
        use_prior: false,
    }
}
fn fixture() -> Catalog {
    let catalog: Catalog = serde_json::from_str(FIXTURE).expect("synthetic fixture parses");
    validate(&catalog).expect("synthetic fixture validates");
    catalog
}

#[test]
fn catalogue_is_bounded_and_rejects_markers_or_bodies() {
    let hostile = FIXTURE.replace(
        "fixture://synthetic/cue-pr-review-v0",
        "</codex_internal_context>",
    );
    assert!(validate(&serde_json::from_str(&hostile).unwrap()).is_err());
    let empty_handle = FIXTURE.replace("review pull request", "!!!");
    assert!(validate(&serde_json::from_str(&empty_handle).unwrap()).is_err());
    let body = FIXTURE.replace("\"risk\"", "\"body\":\"FULL_BODY_SENTINEL\",\"risk\"");
    assert!(serde_json::from_str::<Catalog>(&body).is_err());
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), vec![b'x'; MAX_CATALOG + 1]).unwrap();
    assert_eq!(load(file.path()).err().unwrap(), "oversized");
    let mut cue = fixture().cues.remove(0);
    cue.source = "s".repeat(512);
    cue.title = "界".repeat(170);
    cue.discriminator = "d".repeat(512);
    cue.release_when = "r".repeat(512);
    let fragment = InternalModelContextFragment::new(
        InternalContextSource::from_static("cue_activation"),
        pointer(&cue, "review pull request"),
    );
    assert_eq!(fragment.content_kind().0, "cue_activation.internal_context");
    let rendered = fragment.render();
    assert!(rendered.len() <= 1_200 && rendered.contains(&cue.source));
    assert!(rendered.contains("not instruction") && rendered.contains("No source was hydrated"));
    assert!(
        rendered.contains("report_cue_outcome") && rendered.contains("not a correctness verdict")
    );
    assert!(!rendered.contains("FULL_BODY_SENTINEL"));
    let mut catalog = fixture();
    let positive = query(&[POSITIVE]);
    assert_eq!(
        select_candidate(&catalog, &positive, true).unwrap().0.id,
        "synthetic-pr-review"
    );
    let invalid = query(&[INVALID]);
    assert!(select_candidate(&catalog, &invalid, true).is_none());
    let fused = query(&["please review this pull", "request details"]);
    assert!(select_candidate(&catalog, &fused, true).is_none());
    let stale_avoid = query(&[POSITIVE, INVALID]);
    assert!(select_candidate(&catalog, &stale_avoid, true).is_some());
    let unrelated = query(&["Please compare two spreadsheets", POSITIVE]);
    assert!(select_candidate(&catalog, &unrelated, true).is_none());
    let continued = Query {
        segments: vec!["continue".into(), POSITIVE.into()],
        represented: true,
        use_prior: true,
        ..Query::default()
    };
    let selected = select_candidate(&catalog, &continued, true).unwrap();
    assert_eq!(selected.3, 1);
    let mut tie = catalog.cues[0].clone();
    tie.id = "aaa-deterministic".into();
    catalog.cues.push(tie);
    assert_eq!(
        select_candidate(&catalog, &positive, true).unwrap().0.id,
        "aaa-deterministic"
    );

    let mut lower_score = catalog.cues[0].clone();
    lower_score.id = "eligible-lower-score".into();
    lower_score.task_shape = vec!["review inspect".into()];
    catalog.cues.push(lower_score);
    let mut cooling = query(&[POSITIVE]);
    cooling.cooling.insert("synthetic-pr-review".into());
    cooling.cooling.insert("aaa-deterministic".into());
    assert_eq!(
        select_candidate(&catalog, &cooling, false).unwrap().0.id,
        "eligible-lower-score"
    );
}

#[test]
fn task_context_keeps_two_substantive_requests_and_two_turn_cooldown() {
    let runtime = Runtime::default();
    for item in ["old lighthouse", "middle compass", "recent sextant"] {
        runtime.query(&text(item));
    }
    for acknowledgement in ["continue", "Back :3"] {
        let continued = runtime.query(&text(acknowledgement));
        let context = continued.segments.join("\n");
        assert!(!context.contains("old lighthouse"));
        assert!(context.contains("middle compass") && context.contains("recent sextant"));
        let retained = runtime.0.lock().unwrap();
        assert!(retained.prior.iter().all(|item| item != acknowledgement));
    }
    let punctuation = runtime.query(&text("..."));
    assert!(
        punctuation.represented
            && !punctuation.use_prior
            && select_candidate(&fixture(), &punctuation, true).is_none()
    );
    runtime.emitted("cue");
    let cooling =
        ["one", "two", "three"].map(|item| runtime.query(&text(item)).cooling.contains("cue"));
    assert_eq!(cooling, [true, true, false]);
    let unicode = runtime.query(&text(&"界".repeat(2_000)));
    let bytes = unicode.segments.iter().map(String::len).sum::<usize>();
    let utf8 = unicode
        .segments
        .iter()
        .all(|part| part.is_char_boundary(part.len()));
    assert!(bytes <= 4_096 && utf8);
    let attachment = runtime.query(&[UserInput::Audio {
        audio_url: "x".into(),
    }]);
    assert!(!attachment.represented && select_candidate(&fixture(), &attachment, true).is_none());
    let image_only = runtime.query(&[UserInput::Image {
        image_url: "data:image/png;base64,AA==".into(),
        detail: None,
    }]);
    assert!(!image_only.represented && select_candidate(&fixture(), &image_only, true).is_none());
    let text_and_image = runtime.query(&[
        text(POSITIVE).remove(0),
        UserInput::Image {
            image_url: "data:image/png;base64,AA==".into(),
            detail: None,
        },
    ]);
    assert!(
        text_and_image.represented && select_candidate(&fixture(), &text_and_image, true).is_some()
    );
}

#[test]
fn latest_exclusion_survives_continuation_after_cooldown() {
    let catalog = fixture();
    let runtime = Runtime::default();
    let positive = runtime.query(&text(POSITIVE));
    let selected = select_candidate(&catalog, &positive, /*include_cooling*/ false).unwrap();
    runtime.emitted(&selected.0.id);

    let decisions = [
        "Only write the pull request description",
        "continue",
        "continue",
        "proceed",
    ]
    .map(|request| {
        let query = runtime.query(&text(request));
        (
            query.cooling.contains("synthetic-pr-review"),
            select_candidate(&catalog, &query, /*include_cooling*/ false)
                .map(|selected| selected.0.id.as_str()),
            select_candidate(&catalog, &query, /*include_cooling*/ true)
                .map(|selected| selected.0.id.as_str()),
        )
    });
    assert_eq!(
        decisions,
        [
            (true, None, None),
            (true, None, None),
            (false, None, None),
            (false, None, None),
        ]
    );
}

#[test]
fn newer_positive_replaces_older_exclusion_through_continuation() {
    let catalog = fixture();
    let runtime = Runtime::default();
    let excluded = runtime.query(&text("Only write the pull request description"));
    assert!(select_candidate(&catalog, &excluded, /*include_cooling*/ true).is_none());
    let positive = runtime.query(&text(POSITIVE));
    let selected = select_candidate(&catalog, &positive, /*include_cooling*/ false).unwrap();
    assert_eq!(
        (selected.0.id.as_str(), selected.3),
        ("synthetic-pr-review", 0)
    );
    runtime.emitted(&selected.0.id);

    let decisions = ["continue", "Back :3", "please continue"].map(|request| {
        let query = runtime.query(&text(request));
        let eligible = select_candidate(&catalog, &query, /*include_cooling*/ false)
            .map(|selected| (selected.0.id.as_str(), selected.3));
        let candidate = select_candidate(&catalog, &query, /*include_cooling*/ true)
            .map(|selected| (selected.0.id.as_str(), selected.3));
        (eligible, candidate)
    });
    assert_eq!(
        decisions,
        [
            (None, Some(("synthetic-pr-review", 1))),
            (None, Some(("synthetic-pr-review", 1))),
            (
                Some(("synthetic-pr-review", 1)),
                Some(("synthetic-pr-review", 1)),
            ),
        ]
    );
}

#[test]
fn continuation_does_not_fall_back_past_a_new_unmatched_task() {
    let catalog = fixture();
    let runtime = Runtime::default();
    let no_prior = runtime.query(&text("continue"));
    assert!(select_candidate(&catalog, &no_prior, /*include_cooling*/ true).is_none());
    runtime.query(&text(POSITIVE));

    for request in [
        "Please compare two spreadsheets",
        "back",
        "Back :3",
        "continue",
        "please continue",
        "proceed",
    ] {
        let query = runtime.query(&text(request));
        assert!(
            select_candidate(&catalog, &query, /*include_cooling*/ true).is_none(),
            "unexpected stale cue for {request}"
        );
    }
}

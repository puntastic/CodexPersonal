use super::*;

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
    Query(
        parts.iter().map(|part| (*part).into()).collect(),
        HashSet::new(),
        true,
    )
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
    assert_eq!(load(file.path()).unwrap_err(), "oversized");
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
    assert!(rendered.len() <= 1_000 && rendered.contains(&cue.source));
    assert!(rendered.contains("not instruction") && rendered.contains("No source was hydrated"));
    assert!(!rendered.contains("FULL_BODY_SENTINEL"));
    let mut catalog = fixture();
    let positive = query(&[POSITIVE]);
    assert_eq!(
        select(&catalog, &positive).unwrap().0.id,
        "synthetic-pr-review"
    );
    let invalid = query(&[INVALID]);
    assert!(select(&catalog, &invalid).is_none());
    let fused = query(&["please review this pull", "request details"]);
    assert!(select(&catalog, &fused).is_none());
    let stale_avoid = query(&[POSITIVE, INVALID]);
    assert!(select(&catalog, &stale_avoid).is_some());
    let mut tie = catalog.cues[0].clone();
    tie.id = "aaa-deterministic".into();
    catalog.cues.push(tie);
    assert_eq!(
        select(&catalog, &positive).unwrap().0.id,
        "aaa-deterministic"
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
        let context = continued.0.join("\n");
        assert!(!context.contains("old lighthouse"));
        assert!(context.contains("middle compass") && context.contains("recent sextant"));
        let retained = runtime.0.lock().unwrap();
        assert!(retained.prior.iter().all(|item| item != acknowledgement));
    }
    runtime.emitted("cue");
    let cooling = ["one", "two", "three"].map(|item| runtime.query(&text(item)).1.contains("cue"));
    assert_eq!(cooling, [true, true, false]);
    let unicode = runtime.query(&text(&"界".repeat(2_000)));
    let bytes = unicode.0.iter().map(String::len).sum::<usize>();
    let utf8 = unicode
        .0
        .iter()
        .all(|part| part.is_char_boundary(part.len()));
    assert!(bytes <= 4_096 && utf8);
    let attachment = runtime.query(&[UserInput::Audio {
        audio_url: "x".into(),
    }]);
    assert!(!attachment.2 && select(&fixture(), &attachment).is_none());
}

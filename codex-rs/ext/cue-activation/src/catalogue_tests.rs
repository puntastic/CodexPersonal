use super::*;
use pretty_assertions::assert_eq;

const PROJECT_CATALOGUE: &str = include_str!("../catalogs/project-work-cues.json");

#[derive(Deserialize)]
struct Case {
    name: String,
    request: String,
    expected: Option<String>,
}

#[test]
fn project_catalogue_loads_and_selects_named_cases_without_expanding_pointers() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), PROJECT_CATALOGUE).unwrap();
    let loaded = load(file.path()).expect("the shipped catalogue passes the runtime loader");
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../tests/fixtures/project-cue-cases.json")).unwrap();
    for case in cases {
        let query = Query {
            segments: vec![case.request],
            represented: true,
            ..Query::default()
        };
        let selected = select_candidate(&loaded.catalog, &query, /*include_cooling*/ false);
        assert_eq!(
            selected.as_ref().map(|selection| selection.0.id.clone()),
            case.expected,
            "{}",
            case.name
        );
        if let Some(Selection(cue, handle, _, _)) = selected {
            let fragment = InternalModelContextFragment::new(
                InternalContextSource::from_static("cue_activation"),
                pointer(cue, handle),
            );
            let rendered = fragment.render();
            assert!(rendered.len() <= 1_200, "{}", case.name);
            assert!(rendered.contains(&cue.source), "{}", case.name);
        }
    }
}

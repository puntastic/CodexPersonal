use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test_case::test_case(None, Some(SpawnAgentForkMode::FullHistory); "default")]
#[test_case::test_case(Some(""), Some(SpawnAgentForkMode::FullHistory); "empty default")]
#[test_case::test_case(Some("all"), Some(SpawnAgentForkMode::FullHistory); "all")]
#[test_case::test_case(Some(" ALL "), Some(SpawnAgentForkMode::FullHistory); "trimmed all")]
#[test_case::test_case(Some("none"), None; "none")]
#[test_case::test_case(Some(" NONE "), None; "trimmed none")]
#[test_case::test_case(Some("1"), Some(SpawnAgentForkMode::LastNTurns(1)); "last one")]
#[test_case::test_case(Some(" 3 "), Some(SpawnAgentForkMode::LastNTurns(3)); "last three")]
fn parses_exact_fork_history_mode(input: Option<&str>, expected: Option<SpawnAgentForkMode>) {
    let args: SpawnAgentArgs = serde_json::from_value(json!({
        "message": "inspect this repo",
        "task_name": "worker",
        "fork_turns": input,
    }))
    .expect("spawn arguments");

    assert_eq!(args.fork_mode().expect("valid fork mode"), expected);
}

#[test_case::test_case("0"; "zero")]
#[test_case::test_case("-1"; "negative")]
#[test_case::test_case("1.5"; "fractional")]
#[test_case::test_case("184467440737095516160"; "overflow")]
#[test_case::test_case("banana"; "unknown")]
fn rejects_invalid_fork_count(input: &str) {
    let args: SpawnAgentArgs = serde_json::from_value(json!({
        "message": "inspect this repo",
        "task_name": "worker",
        "fork_turns": input,
    }))
    .expect("spawn arguments");

    assert_eq!(
        args.fork_mode().expect_err("invalid fork mode"),
        FunctionCallError::RespondToModel(
            "fork_turns must be `none`, `all`, or a positive integer string".to_string(),
        ),
    );
}

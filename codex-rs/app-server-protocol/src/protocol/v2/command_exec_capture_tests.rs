use super::CommandExecOutputCaptureEnd;
use super::CommandExecResponse;
use pretty_assertions::assert_eq;
use serde::Deserialize;
use serde_json::json;

#[test]
fn old_command_response_keeps_capture_termination_unknown() {
    let response: CommandExecResponse = serde_json::from_value(json!({
        "exitCode": 0,
        "stdout": "original output",
        "stderr": "",
    }))
    .expect("read an older response");
    assert_eq!(response.stdout_capture_end, None);
    assert_eq!(response.stderr_capture_end, None);
    assert_eq!(response.stdout, "original output");
}

#[test]
fn command_capture_end_round_trips_every_reported_reason() {
    let cases = [
        (CommandExecOutputCaptureEnd::Eof, json!({"type": "eof"})),
        (
            CommandExecOutputCaptureEnd::DrainDeadline,
            json!({"type": "drainDeadline"}),
        ),
        (
            CommandExecOutputCaptureEnd::Cancelled,
            json!({"type": "cancelled"}),
        ),
        (
            CommandExecOutputCaptureEnd::ReadError { os_error: Some(5) },
            json!({"type": "readError", "osError": 5}),
        ),
        (
            CommandExecOutputCaptureEnd::ReadError { os_error: None },
            json!({"type": "readError", "osError": null}),
        ),
    ];
    for (end, wire) in cases {
        assert_eq!(serde_json::to_value(end).expect("serialize end"), wire);
        assert_eq!(
            serde_json::from_value::<CommandExecOutputCaptureEnd>(wire).expect("parse end"),
            end
        );
    }
}

#[test]
fn added_capture_fields_preserve_the_original_response_fields() {
    // This is the former response shape, whose serde reader ignores added fields.
    // It does not assert compatibility with arbitrary strict external decoders.
    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct LegacyResponse {
        exit_code: i32,
        stdout: String,
        stderr: String,
    }
    let response = CommandExecResponse {
        exit_code: 0,
        stdout: "captured prefix".to_string(),
        stderr: String::new(),
        stdout_capture_end: Some(CommandExecOutputCaptureEnd::DrainDeadline),
        stderr_capture_end: Some(CommandExecOutputCaptureEnd::Eof),
    };
    let legacy: LegacyResponse =
        serde_json::from_value(serde_json::to_value(response).expect("serialize current response"))
            .expect("old reader tolerates additive fields");
    assert_eq!(
        legacy,
        LegacyResponse {
            exit_code: 0,
            stdout: "captured prefix".to_string(),
            stderr: String::new(),
        }
    );
}

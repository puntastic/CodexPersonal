use super::*;
use crate::shell::default_user_shell;
use crate::tools::handlers::unified_exec::get_command;
use codex_tools::UnifiedExecShellMode;
use pretty_assertions::assert_eq;
use std::sync::Arc;

#[test]
fn direct_argv_preserves_literal_arguments_without_a_shell() -> anyhow::Result<()> {
    let argv = vec![
        "program.exe", "two words", "", "\"quoted\"", "trailing\\",
        "$name", "%PATH%", "*", "a|b", "a;b", "line\nbreak", "café 東京",
    ];
    let args: ExecCommandArgs = serde_json::from_value(json!({ "argv": argv }))?;
    let resolved = get_command(&args, Arc::new(default_user_shell()), &UnifiedExecShellMode::Direct, /*allow_login_shell*/ true)
        .map_err(anyhow::Error::msg)?;
    assert_eq!(resolved.command, argv);
    assert_eq!(resolved.shell_type, None);
    Ok(())
}

#[test]
fn ambiguous_or_invalid_execution_requests_fail_before_launch() -> anyhow::Result<()> {
    for input in [
        json!({}),
        json!({"cmd":"echo one", "argv":["echo","two"]}),
        json!({"argv":[]}),
        json!({"argv":[""]}),
        json!({"argv":["echo","nul\u{0000}byte"]}),
        json!({"argv":["echo"],"shell":"bash"}),
        json!({"argv":["echo"],"login":true}),
    ] {
        let args: ExecCommandArgs = serde_json::from_value(input)?;
        assert!(get_command(&args, Arc::new(default_user_shell()), &UnifiedExecShellMode::Direct, /*allow_login_shell*/ true).is_err());
    }
    Ok(())
}

#[test]
fn direct_hook_input_carries_display_and_exact_vector() -> anyhow::Result<()> {
    let args: ExecCommandArgs = serde_json::from_value(json!({"argv":["echo","plain"]}))?;
    assert_eq!(args.hook_input().map_err(anyhow::Error::msg)?, json!({"command":"echo plain","argv":["echo","plain"]}));
    Ok(())
}

#[test]
fn script_hook_input_retains_its_existing_shape() -> anyhow::Result<()> {
    let args: ExecCommandArgs = serde_json::from_value(json!({"cmd":"printf hello | sort"}))?;
    assert_eq!(args.hook_input().map_err(anyhow::Error::msg)?, json!({"command":"printf hello | sort"}));
    Ok(())
}

#[test]
fn direct_hook_rewrite_preserves_noncommand_request_fields() -> anyhow::Result<()> {
    let original = json!({"argv":["echo","before"],"workdir":"selected","yield_time_ms":2000}).to_string();
    let args: ExecCommandArgs = serde_json::from_str(&original)?;
    let rewritten = args.rewrite_argv_hook_input(&original, &json!({"argv":["echo","after value"]}))
        .map_err(anyhow::Error::msg)?;
    assert_eq!(serde_json::from_str::<Value>(&rewritten)?, json!({"argv":["echo","after value"],"workdir":"selected","yield_time_ms":2000}));
    Ok(())
}

#[test]
fn direct_hook_text_rewrites_cannot_silently_change_or_bypass_execution() -> anyhow::Result<()> {
    let original = json!({"argv":["echo","before"]}).to_string();
    let args: ExecCommandArgs = serde_json::from_str(&original)?;
    for update in [
        json!({"command":"echo after"}),
        json!({"argv":["echo","before"],"command":"echo after"}),
        json!({"argv":[]}),
        json!({"argv":[42]}),
    ] {
        assert!(args.rewrite_argv_hook_input(&original, &update).is_err());
    }
    let unchanged = args.rewrite_argv_hook_input(&original, &args.hook_input().map_err(anyhow::Error::msg)?)
        .map_err(anyhow::Error::msg)?;
    assert_eq!(serde_json::from_str::<Value>(&unchanged)?, serde_json::from_str::<Value>(&original)?);
    Ok(())
}

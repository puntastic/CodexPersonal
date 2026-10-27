use super::*;
use pretty_assertions::assert_eq;

#[test]
fn desktop_startup_policy_command_parses() {
    let client_info =
        r#"{"name":"codex_desktop","title":"Codex Desktop","version":"26.1021.31345"}"#;
    let cli = MultitoolCli::try_parse_from([
        "codex",
        "app-server",
        "--analytics-default-enabled",
        "--print-startup-config",
        "--startup-client-info",
        client_info,
    ])
    .expect("desktop startup command should parse");
    let Some(Subcommand::AppServer(command)) = cli.subcommand else {
        panic!("expected app-server command");
    };
    assert!(command.print_startup_config);
    assert!(command.analytics_default_enabled);
    assert!(!command.startup_sign_out);
    assert_eq!(
        command.startup_client_info,
        Some(codex_app_server_protocol::ClientInfo {
            name: "codex_desktop".to_owned(),
            title: Some("Codex Desktop".to_owned()),
            version: "26.1021.31345".to_owned(),
        })
    );
}

#[test]
fn startup_options_cannot_silently_affect_a_normal_server() {
    for args in [
        vec!["--startup-sign-out"],
        vec!["--startup-client-info", r#"{"name":"test","version":"1"}"#],
    ] {
        let error = MultitoolCli::try_parse_from(["codex", "app-server"].into_iter().chain(args))
            .expect_err("startup-only options must require the one-shot mode");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }
}

#[test]
fn startup_mode_rejects_transport_and_daemon_options() {
    for args in [
        vec!["--listen", "off"],
        vec!["--stdio"],
        vec!["--remote-control"],
        vec!["--managed-daemon"],
    ] {
        let error = MultitoolCli::try_parse_from(
            ["codex", "app-server", "--print-startup-config"]
                .into_iter()
                .chain(args),
        )
        .expect_err("one-shot startup must not select a server transport");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }
}

#[test]
fn startup_client_info_rejects_invalid_json() {
    let error = MultitoolCli::try_parse_from([
        "codex",
        "app-server",
        "--print-startup-config",
        "--startup-client-info",
        "not-json",
    ])
    .expect_err("invalid client identity must fail parsing");
    assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
}

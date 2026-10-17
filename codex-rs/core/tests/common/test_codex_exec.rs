use codex_login::CODEX_API_KEY_ENV_VAR;
use std::path::Path;
use tempfile::TempDir;
use wiremock::MockServer;

/// Start an Exec fixture that serves SSE and explicitly declines WebSocket upgrades.
/// Keep transport retry/fallback fixtures on their separately configured servers.
pub async fn start_sse_mock_server() -> MockServer {
    let server = crate::responses::start_mock_server().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/responses$"))
        .and(wiremock::matchers::header("upgrade", "websocket"))
        .respond_with(wiremock::ResponseTemplate::new(/*s*/ 426))
        .mount(&server)
        .await;
    server
}

pub struct TestCodexExecBuilder {
    home: TempDir,
    cwd: TempDir,
}

impl TestCodexExecBuilder {
    pub fn cmd(&self) -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::new(
            codex_utils_cargo_bin::cargo_bin("codex-exec")
                .expect("should find binary for codex-exec"),
        );
        cmd.current_dir(self.cwd.path())
            .env("CODEX_HOME", self.home.path())
            .env("CODEX_SQLITE_HOME", self.home.path())
            .env(CODEX_API_KEY_ENV_VAR, "dummy");
        cmd
    }
    pub fn cmd_with_server(&self, server: &MockServer) -> assert_cmd::Command {
        let mut cmd = self.cmd();
        let base = format!("{}/v1", server.uri());
        cmd.arg("-c")
            .arg(format!("openai_base_url={}", toml_string_literal(&base)));
        cmd
    }

    pub fn cwd_path(&self) -> &Path {
        self.cwd.path()
    }
    pub fn home_path(&self) -> &Path {
        self.home.path()
    }
}

fn toml_string_literal(value: &str) -> String {
    serde_json::to_string(value).expect("serialize TOML string literal")
}

pub fn test_codex_exec() -> TestCodexExecBuilder {
    TestCodexExecBuilder {
        home: TempDir::new().expect("create temp home"),
        cwd: TempDir::new().expect("create temp cwd"),
    }
}

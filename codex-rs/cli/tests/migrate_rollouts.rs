use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use codex_protocol::ThreadId;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_rollout::RolloutItem;
use codex_rollout::RolloutLine;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

const APPLY_QUIESCENCE_WARNING: &str = "Close older Codex processes before migration. Byte-exact source files are retained until explicit confirmed-quiescence retirement.";
const THREAD_ID: &str = "123e4567-e89b-12d3-a456-426614174000";
const TIMESTAMP: &str = "2025-01-03T12:00:00Z";

fn codex_command(codex_home: &Path) -> Result<assert_cmd::Command> {
    let mut command = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    command.env("CODEX_HOME", codex_home);
    Ok(command)
}

fn write_legacy_rollout(codex_home: &Path, thread_id: ThreadId) -> Result<()> {
    let directory = codex_home.join("sessions/2025/01/03");
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("rollout-2025-01-03T12-00-00-{thread_id}.jsonl"));
    let mut file = File::create(path)?;
    let line = RolloutLine {
        timestamp: TIMESTAMP.to_string(),
        ordinal: None,
        item: RolloutItem::SessionMeta(SessionMetaLine {
            meta: SessionMeta {
                session_id: thread_id.into(),
                id: thread_id,
                timestamp: TIMESTAMP.to_string(),
                cwd: codex_home.to_path_buf(),
                originator: "migrate-rollouts-cli-test".to_string(),
                cli_version: "0.0.0".to_string(),
                source: SessionSource::Cli,
                model_provider: Some("test-provider".to_string()),
                ..SessionMeta::default()
            },
            git: None,
        }),
    };
    writeln!(file, "{}", serde_json::to_string(&line)?)?;
    Ok(())
}

fn lock_thread_writer(codex_home: &Path, thread_id: ThreadId) -> Result<File> {
    let directory = codex_home.join("thread-writer-locks");
    fs::create_dir_all(&directory)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("{thread_id}.lock")))?;
    file.lock()?;
    Ok(file)
}

#[test]
fn apply_json_keeps_structured_stdout_and_emits_quiescence_warning() -> Result<()> {
    let codex_home = tempfile::tempdir()?;
    let output = codex_command(codex_home.path())?
        .args(["migrate-rollouts", "--apply", "--json"])
        .output()?;

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout)?,
        json!({ "outcomes": [] })
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(APPLY_QUIESCENCE_WARNING),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn busy_apply_is_structured_but_incomplete_and_nonzero() -> Result<()> {
    let codex_home = tempfile::tempdir()?;
    let thread_id = ThreadId::from_string(THREAD_ID).map_err(anyhow::Error::msg)?;
    write_legacy_rollout(codex_home.path(), thread_id)?;
    let _writer_lock = lock_thread_writer(codex_home.path(), thread_id)?;

    let json_output = codex_command(codex_home.path())?
        .args([
            "migrate-rollouts",
            "--apply",
            "--json",
            "--thread",
            THREAD_ID,
        ])
        .output()?;
    assert!(!json_output.status.success());
    let report = serde_json::from_slice::<Value>(&json_output.stdout)?;
    let outcomes = report["outcomes"]
        .as_array()
        .context("migration report should contain outcomes")?;
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0]["thread_id"], THREAD_ID);
    assert_eq!(outcomes[0]["status"], "skipped_busy");
    assert!(
        outcomes[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("already has an active writer"))
    );
    let json_stderr = String::from_utf8_lossy(&json_output.stderr);
    assert!(json_stderr.contains(APPLY_QUIESCENCE_WARNING));
    assert!(json_stderr.contains(
        "one or more rollout migrations were skipped because active writers made the apply pass incomplete"
    ));

    let human_output = codex_command(codex_home.path())?
        .args(["migrate-rollouts", "--apply", "--thread", THREAD_ID])
        .output()?;
    assert!(!human_output.status.success());
    let stdout = String::from_utf8_lossy(&human_output.stdout);
    assert!(stdout.contains("Migration pass incomplete"));
    assert!(!stdout.contains("Migration complete"));
    Ok(())
}

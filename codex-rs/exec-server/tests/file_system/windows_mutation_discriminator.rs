//! Disposable-only filesystem experiment and bounded reporting.
//! Production implementation is unchanged; native fixture effects are VM-only.

#[path = "windows_mutation_discriminator/security.rs"]
mod security;

use crate::support::FileSystemImplementation;
use crate::support::create_file_system_context;
use crate::support::workspace_write_sandbox;
use anyhow::Context;
use anyhow::Result;
use anyhow::ensure;
use codex_exec_server::RemoveOptions;
use codex_exec_server::WriteFileOptions;
use codex_utils_path_uri::PathUri;
use security::AclFacts;
use security::CHILDREN;
use security::CallerSid;
use security::INHERITED;
use security::grant_baseline;
use security::read_facts;
use serde_json::Value;
use serde_json::json;
use std::io;
use std::path::Path;
use std::path::PathBuf;

const MAX_REPORT_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct Report {
    bytes: usize,
    operations: usize,
    operation_failures: usize,
}

impl Report {
    fn emit(&mut self, value: Value) -> Result<()> {
        let text = serde_json::to_string(&value)?;
        let row = format!("FS-DISCRIMINATOR {text}");
        let bytes = row.len() + 1;
        ensure!(
            bytes <= 8192 && self.bytes + bytes <= MAX_REPORT_BYTES,
            "diagnostic output bound exceeded; evidence incomplete"
        );
        eprintln!("{row}");
        self.bytes += bytes;
        Ok(())
    }

    fn snapshot(&mut self, arm: &str, phase: &str, label: &str, path: &Path) -> Result<AclFacts> {
        let facts = read_facts(path).with_context(|| format!("ACL observation {phase}/{label}"))?;
        self.emit(json!({"kind":"acl", "arm":arm, "phase":phase, "path":label, "facts":facts}))?;
        Ok(facts)
    }

    fn operation(
        &mut self,
        arm: &str,
        label: &str,
        operation: &str,
        follow: bool,
        result: io::Result<()>,
        oracle: io::Result<bool>,
    ) -> Result<bool> {
        let success = result.is_ok() && matches!(oracle, Ok(true));
        let error = result.err().map(error_fact);
        let oracle_value = oracle.as_ref().ok().copied();
        let oracle_error = oracle.err().map(error_fact);
        self.operations += 1;
        self.operation_failures += usize::from(!success);
        self.emit(json!({"kind":"operation", "arm":arm, "path":label, "operation":operation, "follow_symlinks":follow, "success":success, "oracle":oracle_value, "oracle_error":oracle_error, "error":error}))?;
        Ok(success)
    }
}

fn error_fact(error: io::Error) -> Value {
    json!({"kind":format!("{:?}", error.kind()), "raw_os_error":error.raw_os_error(),
        "message":error.to_string().chars().take(500).collect::<String>()})
}

#[derive(Clone, Copy)]
enum Baseline {
    Inherited,
    QualifiedUser,
}

async fn run_arm(
    parent: &Path,
    baseline: Baseline,
    caller: &CallerSid,
    report: &mut Report,
) -> Result<()> {
    let arm = match baseline {
        Baseline::Inherited => "inherited-baseline",
        Baseline::QualifiedUser => "qualified-user-baseline",
    };
    let directory = tempfile::Builder::new()
        .prefix("codex-fs-discriminator-")
        .tempdir_in(parent)?;
    let root = directory.path().canonicalize()?;
    ensure!(
        root.parent() == Some(parent)
            && !directory
                .path()
                .symlink_metadata()?
                .file_type()
                .is_symlink(),
        "fixture root escaped its owned parent"
    );
    ensure!(
        std::fs::read_dir(&root)?.next().transpose()?.is_none(),
        "fixture root was not empty"
    );
    let user = caller.string()?;
    let before = report.snapshot(arm, "before-baseline", "root", &root)?;
    report.emit(json!({"kind":"scope", "arm":arm, "fixture_id":directory.path().file_name().context("fixture directory name missing")?.to_string_lossy(), "parent":"standard fixture temp directory; canonical direct-parent identity and RUNNER_TEMP containment checked", "actual_token_user":user, "fresh_empty_direct_child":true, "root_user_oi_ci_full_access":before.full_user_grant(&user, CHILDREN)}))?;
    if matches!(baseline, Baseline::QualifiedUser) {
        grant_baseline(&root, caller, &before)?;
        let after = report.snapshot(arm, "after-baseline-before-children", "root", &root)?;
        ensure!(
            after.full_user_grant(&user, CHILDREN),
            "root lacks a qualifying OI|CI full-access allow ACE for the actual user"
        );
    }
    let context = create_file_system_context(FileSystemImplementation::Local).await?;
    let sandbox = workspace_write_sandbox(root.clone());
    let mut files = Vec::new();
    for follow in [false, true] {
        let label = format!("parent-before-{follow}.txt");
        let path = root.join(&label);
        std::fs::write(&path, b"parent seed")
            .with_context(|| format!("seed {label} before preparation"))?;
        let facts = report.snapshot(arm, "before-helper-preparation", &label, &path)?;
        if matches!(baseline, Baseline::QualifiedUser) {
            ensure!(
                facts.full_user_grant(&user, INHERITED),
                "parent seed did not inherit qualified baseline"
            );
        }
        files.push((label, path, follow));
    }
    for follow in [false, true] {
        let label = format!("helper-created-{follow}.txt");
        let path = root.join(&label);
        let result = context
            .file_system
            .write_file(
                &PathUri::from_host_native_path(&path)?,
                b"helper seed".to_vec(),
                WriteFileOptions {
                    follow_symlinks: follow,
                },
                Some(&sandbox),
            )
            .await;
        let oracle = std::fs::read(&path).map(|bytes| bytes == b"helper seed");
        if report.operation(arm, &label, "create", follow, result, oracle)? {
            files.push((label, path, follow));
        } else {
            report.emit(json!({"kind":"unavailable-case", "arm":arm, "path":label, "reason":"helper creation/data oracle failed; overwrite and remove not attempted for this path"}))?;
        }
    }
    report.snapshot(arm, "after-helper-preparation", "root", &root)?;
    for (label, path, _) in &files {
        report.snapshot(arm, "after-helper-preparation", label, path)?;
    }
    for follow in [false, true] {
        let label = format!("parent-after-{follow}.txt");
        let path = root.join(&label);
        std::fs::write(&path, b"parent seed")
            .with_context(|| format!("seed {label} after preparation"))?;
        let facts = report.snapshot(arm, "parent-seeded-after-preparation", &label, &path)?;
        if matches!(baseline, Baseline::QualifiedUser) {
            ensure!(
                facts.full_user_grant(&user, INHERITED),
                "late parent seed did not inherit qualified baseline"
            );
        }
        files.push((label, path, follow));
    }
    for (label, path, follow) in files {
        let uri = PathUri::from_host_native_path(&path)?;
        let result = context
            .file_system
            .write_file(
                &uri,
                b"overwritten".to_vec(),
                WriteFileOptions {
                    follow_symlinks: follow,
                },
                Some(&sandbox),
            )
            .await;
        let oracle = std::fs::read(&path).map(|bytes| bytes == b"overwritten");
        report.operation(arm, &label, "overwrite", follow, result, oracle)?;
        report.snapshot(arm, "after-overwrite-before-remove", &label, &path)?;
        let result = context
            .file_system
            .remove(
                &uri,
                RemoveOptions {
                    recursive: false,
                    force: false,
                    follow_symlinks: follow,
                },
                Some(&sandbox),
            )
            .await;
        let absent = match std::fs::symlink_metadata(&path) {
            Ok(_) => Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Err(error) => Err(error),
        };
        report.operation(arm, &label, "remove", follow, result, absent)?;
    }
    report.snapshot(arm, "after-operations", "root", &root)?;
    // Only ordinary cleanup of this owned TempDir; never forced or by PID.
    directory
        .close()
        .context("ordinary discriminator fixture cleanup")?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a fresh disposable GitHub-hosted Windows VM"]
async fn parent_and_helper_files_compare_baselines_and_follow_modes() -> Result<()> {
    ensure!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref() == Ok("github-hosted")
            && std::env::var("RUNNER_OS").as_deref() == Ok("Windows")
            && std::env::var("GITHUB_REPOSITORY").as_deref() == Ok("puntastic/CodexPersonal"),
        "refusing filesystem discriminator outside the disposable GitHub-hosted Windows repository job"
    );
    let mut report = Report::default();
    report.emit(json!({"kind":"predictions-before-observation", "baseline":"Missing ordinary-user baseline is supported only if qualified parent-seeded mutations recover while root/child baseline readbacks qualify; otherwise grant/inheritance/native-operation explanations remain live.", "limits":"Method-level results only: no helper-internal syscall stage, restricted-token query, effective AccessCheck oracle, isolation claim or legacy-policy repair. Original regressions remain unchanged."}))?;
    let runner_temp =
        PathBuf::from(std::env::var_os("RUNNER_TEMP").context("RUNNER_TEMP missing")?)
            .canonicalize()?;
    // Match the original TempDir fixtures' ancestry, rather than changing the
    // inherited-ACL baseline by moving directly under RUNNER_TEMP.
    let parent = std::env::temp_dir().canonicalize()?;
    ensure!(
        parent.starts_with(&runner_temp),
        "fixture temp directory escaped RUNNER_TEMP"
    );
    let caller = CallerSid::read()?;
    let mut incomplete_arms = 0;
    for baseline in [Baseline::Inherited, Baseline::QualifiedUser] {
        if let Err(error) = run_arm(&parent, baseline, &caller, &mut report).await {
            incomplete_arms += 1;
            let arm = match baseline {
                Baseline::Inherited => "inherited-baseline",
                Baseline::QualifiedUser => "qualified-user-baseline",
            };
            report.emit(json!({"kind":"incomplete-arm", "arm":arm, "error":format!("{error:#}").chars().take(1000).collect::<String>()}))?;
        }
    }
    report.emit(json!({"kind":"summary", "operations_observed":report.operations, "operations_not_reached":28usize.saturating_sub(report.operations), "operation_failures":report.operation_failures, "incomplete_arms":incomplete_arms, "passed_is_not_isolation":true}))?;
    ensure!(
        report.operation_failures == 0 && incomplete_arms == 0,
        "discriminator retained failed operations or incomplete arms; inspect FS-DISCRIMINATOR rows"
    );
    Ok(())
}

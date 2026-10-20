#![cfg(windows)]
#![allow(clippy::expect_used)]

mod common;

#[path = "file_system/shared.rs"]
mod shared;
#[path = "file_system/support.rs"]
mod support;
#[path = "file_system/windows_fixture_security.rs"]
mod windows_fixture_security;
#[path = "file_system/windows_mutation_discriminator.rs"]
mod windows_mutation_discriminator;

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use codex_exec_server::CreateDirectoryOptions;
use codex_exec_server::FileSystemSandboxContext;
use codex_exec_server::GetMetadataOptions;
use codex_exec_server::ReadFileOptions;
use codex_exec_server::RemoveOptions;
use codex_exec_server::WindowsSandboxSelection;
use codex_exec_server::WriteFileOptions;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::SandboxPolicy;
use codex_sandboxing::SandboxType;
use codex_utils_path_uri::PathUri;
use codex_windows_sandbox_test_support::WindowsSandboxAccountTestGuard;
use futures::TryStreamExt;
use pretty_assertions::assert_eq;
use test_case::test_case;
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::time::timeout;
use uuid::Uuid;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::shared::io_error_with_phase;
use crate::support::FileSystemImplementation;
use crate::support::create_file_system_context;
use crate::support::is_unsupported_restricted_token_host;
use crate::support::workspace_write_sandbox;
use crate::windows_fixture_security::FixtureUserBaseline;

fn create_directory_junction(target: &Path, alias: &Path) -> Result<()> {
    let output = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(alias)
        .arg(target)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "mklink /J failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[test_case(FileSystemImplementation::Local ; "local")]
#[test_case(FileSystemImplementation::Remote ; "remote")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_system_canonicalize_resolves_directory_junction(
    implementation: FileSystemImplementation,
) -> Result<()> {
    shared::assert_canonicalize_resolves_directory_alias(implementation, create_directory_junction)
        .await
}

#[test_case(FileSystemImplementation::Local ; "local")]
#[test_case(FileSystemImplementation::Remote ; "remote")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_system_sandboxed_canonicalize_resolves_directory_junction(
    implementation: FileSystemImplementation,
) -> Result<()> {
    shared::assert_sandboxed_canonicalize_resolves_directory_alias(
        implementation,
        create_directory_junction,
    )
    .await
}

#[test_case(FileSystemImplementation::Local ; "local")]
#[test_case(FileSystemImplementation::Remote ; "remote")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_system_operations_can_reject_junctions_in_any_path_component(
    implementation: FileSystemImplementation,
) -> Result<()> {
    let context = create_file_system_context(implementation)
        .await
        .context("phase=setup/filesystem-context")?;
    let tmp =
        tempfile::TempDir::new().map_err(|error| io_error_with_phase("setup/tempdir", error))?;
    let baseline =
        FixtureUserBaseline::establish(&tmp).context("phase=setup/ordinary-user-baseline")?;
    let real = tmp.path().join("real");
    std::fs::create_dir(&real)
        .map_err(|error| io_error_with_phase("setup/real-directory", error))?;
    baseline.verify_inherited(&real)?;
    let existing = real.join("existing.txt");
    std::fs::write(&existing, "unchanged")
        .map_err(|error| io_error_with_phase("setup/seed-existing-file", error))?;
    baseline.verify_inherited(&existing)?;
    let removable = real.join("removable.txt");
    std::fs::write(&removable, "keep")
        .map_err(|error| io_error_with_phase("setup/seed-removable-file", error))?;
    baseline.verify_inherited(&removable)?;
    let directory_junction = tmp.path().join("directory-junction");
    create_directory_junction(&real, &directory_junction)
        .context("phase=setup/directory-junction")?;

    let no_follow_read = ReadFileOptions {
        follow_symlinks: false,
    };
    let no_follow_write = WriteFileOptions {
        follow_symlinks: false,
    };
    let no_follow_metadata = GetMetadataOptions {
        follow_symlinks: false,
    };
    let no_follow_create = CreateDirectoryOptions {
        recursive: true,
        follow_symlinks: false,
    };
    let no_follow_remove = RemoveOptions {
        recursive: false,
        force: false,
        follow_symlinks: false,
    };
    let uri = |path: &Path| PathUri::from_host_native_path(path);

    assert_eq!(
        context
            .file_system
            .read_file(&uri(&existing)?, no_follow_read, /*sandbox*/ None,)
            .await
            .map_err(|error| io_error_with_phase("read/unsandboxed-existing-file", error))?,
        b"unchanged"
    );

    let file_link_target = real.join("file-link-target.txt");
    std::fs::write(&file_link_target, "target")
        .map_err(|error| io_error_with_phase("setup/seed-file-link-target", error))?;
    let file_link = tmp.path().join("file-link.txt");
    if std::os::windows::fs::symlink_file(&file_link_target, &file_link).is_ok() {
        assert!(
            context
                .file_system
                .write_file(
                    &uri(&file_link)?,
                    b"changed".to_vec(),
                    no_follow_write,
                    /*sandbox*/ None,
                )
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(&file_link_target)
                .map_err(|error| io_error_with_phase("verify/file-link-target", error))?,
            "target"
        );
    }

    assert!(
        context
            .file_system
            .read_file(
                &uri(&directory_junction.join("existing.txt"))?,
                no_follow_read,
                /*sandbox*/ None,
            )
            .await
            .is_err()
    );
    assert!(
        context
            .file_system
            .write_file(
                &uri(&directory_junction.join("existing.txt"))?,
                b"changed".to_vec(),
                no_follow_write,
                /*sandbox*/ None,
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(&existing)
            .map_err(|error| io_error_with_phase("verify/unsandboxed-junction-target", error))?,
        "unchanged"
    );
    assert!(
        context
            .file_system
            .get_metadata(
                &uri(&directory_junction)?,
                no_follow_metadata,
                /*sandbox*/ None,
            )
            .await
            .is_err()
    );
    let directory_metadata = context
        .file_system
        .get_metadata(&uri(&real)?, no_follow_metadata, /*sandbox*/ None)
        .await
        .map_err(|error| io_error_with_phase("metadata/unsandboxed-real-directory", error))?;
    assert!(directory_metadata.is_directory);
    assert!(
        context
            .file_system
            .create_directory(
                &uri(&directory_junction.join("created"))?,
                no_follow_create,
                /*sandbox*/ None,
            )
            .await
            .is_err()
    );
    assert!(!real.join("created").exists());
    assert!(
        context
            .file_system
            .remove(
                &uri(&directory_junction.join("removable.txt"))?,
                no_follow_remove,
                /*sandbox*/ None,
            )
            .await
            .is_err()
    );
    assert!(removable.exists());
    assert!(
        context
            .file_system
            .remove(
                &uri(&directory_junction)?,
                no_follow_remove,
                /*sandbox*/ None,
            )
            .await
            .is_err()
    );
    assert!(
        directory_junction
            .symlink_metadata()
            .map_err(|error| io_error_with_phase("verify/directory-junction", error))?
            .file_type()
            .is_symlink()
    );

    let sandbox = workspace_write_sandbox(tmp.path().to_path_buf());
    let read_result = context
        .file_system
        .read_file(&uri(&existing)?, no_follow_read, Some(&sandbox))
        .await;
    assert_eq!(
        read_result.map_err(|error| io_error_with_phase("read/sandboxed-existing-file", error))?,
        b"unchanged"
    );
    let write_result = context
        .file_system
        .write_file(
            &uri(&existing)?,
            b"unchanged".to_vec(),
            no_follow_write,
            Some(&sandbox),
        )
        .await;
    if is_unsupported_restricted_token_host(&write_result) {
        eprintln!(
            "phase=write/sandboxed-existing-file, outcome=unsupported-restricted-token, mode={implementation}"
        );
        return Ok(());
    }
    write_result.map_err(|error| io_error_with_phase("write/sandboxed-existing-file", error))?;
    assert!(
        context
            .file_system
            .read_file(
                &uri(&directory_junction.join("existing.txt"))?,
                no_follow_read,
                Some(&sandbox),
            )
            .await
            .is_err()
    );
    assert!(
        context
            .file_system
            .write_file(
                &uri(&directory_junction.join("existing.txt"))?,
                b"changed".to_vec(),
                no_follow_write,
                Some(&sandbox),
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(&existing)
            .map_err(|error| io_error_with_phase("verify/sandboxed-junction-target", error))?,
        "unchanged"
    );
    assert!(
        context
            .file_system
            .get_metadata(
                &uri(&directory_junction)?,
                no_follow_metadata,
                Some(&sandbox),
            )
            .await
            .is_err()
    );
    assert!(
        context
            .file_system
            .create_directory(
                &uri(&directory_junction.join("sandbox-created"))?,
                no_follow_create,
                Some(&sandbox),
            )
            .await
            .is_err()
    );
    assert!(!real.join("sandbox-created").exists());
    assert!(
        context
            .file_system
            .remove(
                &uri(&directory_junction.join("removable.txt"))?,
                no_follow_remove,
                Some(&sandbox),
            )
            .await
            .is_err()
    );
    assert!(removable.exists());

    Ok(())
}

#[test_case(FileSystemImplementation::Local ; "local")]
#[test_case(FileSystemImplementation::Remote ; "remote")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_system_no_follow_operations_reject_named_pipes(
    implementation: FileSystemImplementation,
) -> Result<()> {
    let context = create_file_system_context(implementation).await?;
    let pipe_name = format!("codex-fs-no-follow-{}", Uuid::new_v4());
    let server_path = format!(r"\\.\pipe\{pipe_name}");
    let client_path = format!(r"\\localhost\pipe\{pipe_name}");
    let _pipe = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&server_path)?;

    let error = timeout(
        Duration::from_secs(1),
        context.file_system.read_file(
            &PathUri::from_host_native_path(Path::new(&client_path))?,
            ReadFileOptions {
                follow_symlinks: false,
            },
            /*sandbox*/ None,
        ),
    )
    .await
    .expect("strict named-pipe read must not hang")
    .expect_err("strict named-pipe read must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

    let pipe_name = format!("codex-fs-no-follow-write-{}", Uuid::new_v4());
    let server_path = format!(r"\\.\pipe\{pipe_name}");
    let client_path = format!(r"\\localhost\pipe\{pipe_name}");
    let _pipe = ServerOptions::new()
        .first_pipe_instance(true)
        .create(&server_path)?;
    timeout(
        Duration::from_secs(1),
        context.file_system.write_file(
            &PathUri::from_host_native_path(Path::new(&client_path))?,
            b"must not be written".to_vec(),
            WriteFileOptions {
                follow_symlinks: false,
            },
            /*sandbox*/ None,
        ),
    )
    .await
    .expect("strict named-pipe write must not hang")
    .expect_err("strict named-pipe write must be rejected");
    Ok(())
}

#[test_case(SandboxType::WindowsRestrictedToken; "restricted_token")]
#[test_case(SandboxType::WindowsMxc; "mxc")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_system_remote_fs_helper_respects_windows_sandbox_write_policy(
    sandbox_type: SandboxType,
) -> Result<()> {
    let context = create_file_system_context(FileSystemImplementation::Remote).await?;
    let file_system = context.file_system;
    let tmp = tempfile::TempDir::new()?;
    let readonly_dir = tmp.path().join("readonly");
    std::fs::create_dir_all(&readonly_dir)?;

    let mut sandbox = read_only_sandbox_for_cwd(readonly_dir.clone())?;
    match sandbox_type {
        SandboxType::WindowsRestrictedToken => {
            sandbox.windows_sandbox_selection = WindowsSandboxSelection::RestrictedToken;
        }
        SandboxType::WindowsMxc => {
            sandbox.windows_sandbox_selection = WindowsSandboxSelection::Mxc;
        }
        SandboxType::None | SandboxType::MacosSeatbelt | SandboxType::LinuxSeccomp => {
            anyhow::bail!("expected a Windows sandbox type")
        }
    }

    let readable_file = readonly_dir.join("readable.txt");
    std::fs::write(&readable_file, b"readable")?;
    let read_result = file_system
        .read_file(
            &PathUri::from_host_native_path(&readable_file)?,
            ReadFileOptions::default(),
            Some(&sandbox),
        )
        .await;
    assert_eq!(read_result?, b"readable");

    let blocked_file = readonly_dir.join("blocked.txt");
    if sandbox_type == SandboxType::WindowsMxc && !codex_sandboxing::windows_mxc_available() {
        let error = file_system
            .write_file(
                &PathUri::from_host_native_path(&blocked_file)?,
                b"blocked".to_vec(),
                WriteFileOptions::default(),
                Some(&sandbox),
            )
            .await
            .expect_err("unavailable MXC must fail closed");
        assert_eq!(
            (error.kind(), error.to_string()),
            (
                std::io::ErrorKind::InvalidInput,
                "failed to prepare fs sandbox: failed to prepare MXC sandbox: native MXC is unavailable on this executor".to_owned(),
            )
        );
        assert!(!blocked_file.exists());
        return Ok(());
    }

    let write_result = file_system
        .write_file(
            &PathUri::from_host_native_path(&blocked_file)?,
            b"blocked".to_vec(),
            WriteFileOptions::default(),
            Some(&sandbox),
        )
        .await;
    // Some local Windows hosts cannot create restricted tokens. Reaching that
    // error still proves the write went through the Windows sandbox launcher.
    if sandbox_type == SandboxType::WindowsRestrictedToken
        && is_unsupported_restricted_token_host(&write_result)
    {
        assert!(!blocked_file.exists());
        return Ok(());
    }
    let error = write_result.expect_err("write outside the sandbox should fail");
    assert!(
        !blocked_file.exists(),
        "sandboxed fs helper must not create blocked file after error: {error}"
    );

    Ok(())
}

/// An elevated filesystem helper must enforce relative deny globs from the policy cwd.
#[test_case(FileSystemImplementation::Local ; "local")]
#[test_case(FileSystemImplementation::Remote ; "remote")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial(remote_exec_server)]
async fn file_system_elevated_relative_read_denial_uses_policy_cwd(
    implementation: FileSystemImplementation,
) -> Result<()> {
    let _account_guard = WindowsSandboxAccountTestGuard::acquire()?;
    // Both implementations re-enter this test binary; the elevated backend finds its helpers
    // next to that binary, while Cargo and Bazel provide them separately.
    let test_exe = std::env::current_exe()?;
    let resources = test_exe
        .parent()
        .context("Windows test executable should have a parent directory")?
        .join("codex-resources");
    if let Err(error) = std::fs::create_dir_all(&resources)
        && !(error.kind() == std::io::ErrorKind::PermissionDenied && resources.is_dir())
    {
        return Err(error).context("create Windows sandbox test resources");
    }
    for name in ["codex-windows-sandbox-setup", "codex-command-runner"] {
        let source = codex_utils_cargo_bin::cargo_bin(name)?;
        let destination = resources.join(Path::new(name).with_extension("exe"));
        if let Err(error) = codex_utils_cargo_bin::copy_executable(&source, &destination)
            && !(error.kind() == std::io::ErrorKind::PermissionDenied && destination.is_file())
        {
            return Err(error).with_context(|| format!("stage Windows sandbox helper {name}"));
        }
    }
    let context = create_file_system_context(implementation).await?;
    let tmp = tempfile::TempDir::new()?;
    let policy_cwd = tmp.path().join("checkout");
    let selected_files = policy_cwd.join("files");
    let other_files = tmp.path().join("files");
    std::fs::create_dir_all(&selected_files)?;
    std::fs::create_dir(&other_files)?;
    let allowed_neighbor = selected_files.join("allowed.txt");
    let denied = selected_files.join("blocked.env");
    let same_name_outside = other_files.join("blocked.env");
    std::fs::write(&allowed_neighbor, b"allowed neighbor")?;
    std::fs::write(&denied, b"denied")?;
    std::fs::write(&same_name_outside, b"allowed outside")?;

    let cwd = PathUri::from_host_native_path(&policy_cwd)?;
    let policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry::new(
            FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            FileSystemAccessMode::Read,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::Path {
                path: PathUri::from_host_native_path(tmp.path())?,
            },
            FileSystemAccessMode::Write,
        ),
        FileSystemSandboxEntry::new(
            FileSystemPath::GlobPattern {
                pattern: "files/*.env".to_string(),
            },
            FileSystemAccessMode::Deny,
        ),
    ]);
    let mut sandbox = FileSystemSandboxContext::from_permission_profile(
        PermissionProfile::from_runtime_permissions(&policy, NetworkSandboxPolicy::Restricted),
        cwd,
    );
    sandbox.windows_sandbox_selection = WindowsSandboxSelection::Elevated;

    let file_system = &context.file_system;
    let allowed_neighbor = file_system
        .read_file(
            &PathUri::from_host_native_path(&allowed_neighbor)?,
            ReadFileOptions::default(),
            Some(&sandbox),
        )
        .await?;
    let same_name_outside = file_system
        .read_file(
            &PathUri::from_host_native_path(&same_name_outside)?,
            ReadFileOptions::default(),
            Some(&sandbox),
        )
        .await?;
    let denied = file_system
        .read_file(
            &PathUri::from_host_native_path(&denied)?,
            ReadFileOptions::default(),
            Some(&sandbox),
        )
        .await
        .expect_err("read matching the policy-cwd denial must be rejected");
    assert_eq!(
        (
            allowed_neighbor.as_slice(),
            same_name_outside.as_slice(),
            denied.kind(),
        ),
        (
            b"allowed neighbor".as_slice(),
            b"allowed outside".as_slice(),
            std::io::ErrorKind::InvalidInput,
        )
    );
    assert!(
        denied.to_string().contains("Access is denied"),
        "expected Windows access denial, got: {denied}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_system_private_desktop_survives_helper_exits_and_separates_permissions() -> Result<()>
{
    let context = create_file_system_context(FileSystemImplementation::Local)
        .await
        .context("phase=setup/filesystem-context")?;
    let file_system = context.file_system;
    let tmp =
        tempfile::TempDir::new().map_err(|error| io_error_with_phase("setup/tempdir", error))?;
    let baseline =
        FixtureUserBaseline::establish(&tmp).context("phase=setup/ordinary-user-baseline")?;
    let path = tmp.path().join("contents.txt");
    std::fs::write(&path, b"initial")
        .map_err(|error| io_error_with_phase("setup/seed-desktop-test-file", error))?;
    baseline.verify_inherited(&path)?;
    let uri = PathUri::from_host_native_path(&path)?;
    let sandbox = workspace_write_sandbox(tmp.path().to_path_buf());
    let before = process_private_desktops().context("phase=private-desktop/snapshot-before")?;
    let write = file_system
        .write_file(
            &uri,
            b"initial".to_vec(),
            WriteFileOptions::default(),
            Some(&sandbox),
        )
        .await;
    if is_unsupported_restricted_token_host(&write) {
        eprintln!(
            "phase=private-desktop/initial-existing-write, outcome=unsupported-restricted-token"
        );
        return Ok(());
    }
    write.map_err(|error| io_error_with_phase("private-desktop/initial-existing-write", error))?;

    // Ownership must outlive each helper; a desktop held only by the helper disappears here.
    let warmed = process_private_desktops().context("phase=private-desktop/snapshot-warmed")?;
    assert_eq!(warmed.difference(&before).count(), 1);
    for contents in ["updated", "updated again"] {
        file_system
            .write_file(
                &uri,
                contents.as_bytes().to_vec(),
                WriteFileOptions::default(),
                Some(&sandbox),
            )
            .await
            .map_err(|error| io_error_with_phase("private-desktop/rewrite-existing-file", error))?;
        assert_eq!(
            process_private_desktops().context("phase=private-desktop/snapshot-after-write")?,
            warmed
        );
        assert_eq!(
            file_system
                .read_file(&uri, ReadFileOptions::default(), Some(&sandbox))
                .await
                .map_err(|error| io_error_with_phase("private-desktop/read-file", error))?,
            contents.as_bytes()
        );
        assert_eq!(
            process_private_desktops().context("phase=private-desktop/snapshot-after-read")?,
            warmed
        );
        assert!(
            file_system
                .get_metadata(&uri, GetMetadataOptions::default(), Some(&sandbox))
                .await
                .map_err(|error| io_error_with_phase("private-desktop/metadata", error))?
                .is_file
        );
        assert_eq!(
            process_private_desktops().context("phase=private-desktop/snapshot-after-metadata")?,
            warmed
        );
        let chunks = file_system
            .read_file_stream(&uri, Some(&sandbox))
            .await
            .map_err(|error| io_error_with_phase("private-desktop/open-read-stream", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| io_error_with_phase("private-desktop/read-stream", error))?;
        assert_eq!(chunks.concat(), contents.as_bytes());
        assert_eq!(
            process_private_desktops().context("phase=private-desktop/snapshot-after-stream")?,
            warmed
        );
    }

    let mut readonly = read_only_sandbox_for_cwd(tmp.path().to_path_buf())
        .context("phase=setup/readonly-policy")?;
    readonly.windows_sandbox_selection = WindowsSandboxSelection::RestrictedToken;
    file_system
        .write_file(
            &uri,
            b"blocked".to_vec(),
            WriteFileOptions::default(),
            Some(&readonly),
        )
        .await
        .expect_err("read-only filesystem requests must reject writes");
    assert_eq!(
        std::fs::read(&path)
            .map_err(|error| io_error_with_phase("verify/readonly-write-denied", error))?,
        b"updated again"
    );
    let separated = process_private_desktops()
        .context("phase=private-desktop/snapshot-separated-permissions")?;
    assert!(warmed.is_subset(&separated));
    assert_eq!(separated.difference(&warmed).count(), 1);
    assert_eq!(
        file_system
            .read_file(&uri, ReadFileOptions::default(), Some(&readonly))
            .await
            .map_err(|error| io_error_with_phase("private-desktop/readonly-read", error))?,
        b"updated again"
    );
    assert_eq!(
        process_private_desktops().context("phase=private-desktop/snapshot-after-readonly-read")?,
        separated
    );
    Ok(())
}

fn process_private_desktops() -> Result<BTreeSet<String>> {
    // Query this process so other tests' private desktops cannot affect the assertions.
    // Native layout: https://github.com/winsiderss/phnt/blob/master/ntpsapi.h
    #[repr(C)]
    struct HandleEntry {
        handle: HANDLE,
        _handle_count: usize,
        _pointer_count: usize,
        _granted_access: u32,
        _object_type_index: u32,
        _handle_attributes: u32,
        _reserved: u32,
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationProcess(
            process: HANDLE,
            class: u32,
            information: *mut c_void,
            length: u32,
            return_length: *mut u32,
        ) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetUserObjectInformationW(
            object: HANDLE,
            index: i32,
            information: *mut c_void,
            length: u32,
            length_needed: *mut u32,
        ) -> i32;
    }
    let mut snapshot = vec![0usize; 8192];
    let bytes = std::mem::size_of_val(snapshot.as_slice());
    let status = unsafe {
        NtQueryInformationProcess(
            GetCurrentProcess(),
            /*class*/ 51,
            snapshot.as_mut_ptr().cast(),
            bytes as u32,
            std::ptr::null_mut(),
        )
    };
    anyhow::ensure!(status >= 0, "process handle query failed: {status:#x}");
    let count = snapshot[0];
    anyhow::ensure!(
        count <= (bytes - 2 * size_of::<usize>()) / size_of::<HandleEntry>(),
        "process handle snapshot exceeds its buffer"
    );
    let entries = unsafe {
        std::slice::from_raw_parts(snapshot.as_ptr().add(2).cast::<HandleEntry>(), count)
    };
    let mut desktops = BTreeSet::new();
    for entry in entries {
        let mut name = [0u16; 64];
        let mut length_needed = 0;
        if unsafe {
            GetUserObjectInformationW(
                entry.handle,
                /*index*/ 2,
                name.as_mut_ptr().cast(),
                std::mem::size_of_val(&name) as u32,
                &mut length_needed,
            )
        } != 0
        {
            let end = name
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(name.len());
            let name = String::from_utf16(&name[..end])?;
            if name.starts_with("CodexSandboxDesktop-") {
                desktops.insert(name);
            }
        }
    }
    Ok(desktops)
}

fn read_only_sandbox_for_cwd(cwd: std::path::PathBuf) -> Result<FileSystemSandboxContext> {
    Ok(FileSystemSandboxContext::from_legacy_sandbox_policy(
        SandboxPolicy::new_read_only_policy(),
        PathUri::from_host_native_path(cwd)?,
    )?)
}

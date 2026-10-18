//! Native console oracle and observations for the legacy capture lifecycle test.

use super::capture_result_diagnostics;
use super::legacy_process_test_guard;
use super::powershell_literal;
use super::pwsh_path;
use super::sandbox_cwd;
use super::sandbox_home;
use super::start_powershell_child;
use super::wait_for_path;
use super::wait_for_process_exit;
use super::workspace_roots_for;
use crate::run_windows_sandbox_capture;
use codex_protocol::models::PermissionProfile;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use tempfile::TempDir;
use windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
use windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS;
use windows_sys::Win32::System::Console::AttachConsole;
use windows_sys::Win32::System::Console::FreeConsole;
use windows_sys::Win32::System::Console::GetConsoleWindow;
use windows_sys::Win32::System::Threading::GetExitCodeProcess;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows_sys::Win32::System::Threading::PROCESS_SYNCHRONIZE;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

const CONSOLE_PROBE_TEST: &str =
    "unified_exec::tests::legacy_capture_fixture::powershell_parent_has_no_console";
const CONSOLE_PROBE_ENV: &str = "CODEX_TEST_LEGACY_CONSOLE_PROBE";
const CONSOLE_PROBE_MARKER: &str = "LEGACY-NATIVE-NO-CONSOLE";

struct CaptureFinished<'a>(&'a AtomicBool);

impl Drop for CaptureFinished<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn process_exit_code(process: &OwnedHandle) -> io::Result<u32> {
    let mut exit_code = 0;
    let queried = unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut exit_code) };
    if queried == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(exit_code)
    }
}

pub(super) fn console_probe_env() -> HashMap<String, String> {
    HashMap::from([(CONSOLE_PROBE_ENV.to_string(), "1".to_string())])
}

pub(super) fn powershell_no_console_probe(stdio_dir: &Path) -> String {
    let executable = std::env::current_exe().expect("locate native console probe");
    let stdout = powershell_literal(&stdio_dir.join("console-probe.stdout"));
    let stderr = powershell_literal(&stdio_dir.join("console-probe.stderr"));
    // Check the marker as well as the exit status: libtest succeeds when a
    // misspelled exact filter runs no tests, and a skipped probe must not pass.
    // Do not use PowerShell's direct native-command path (&): it can allocate a
    // hidden console in PowerShell itself, changing the state being observed.
    format!(
        "$consoleProbe = Start-Process -ErrorAction Stop -WindowStyle Hidden -FilePath '{}' -ArgumentList '{CONSOLE_PROBE_TEST}','--exact','--ignored','--nocapture' -RedirectStandardOutput '{stdout}' -RedirectStandardError '{stderr}' -Wait -PassThru; if ($consoleProbe.ExitCode -ne 0 -or ((Get-Content -ErrorAction Stop -Raw -LiteralPath '{stdout}') -notmatch '{CONSOLE_PROBE_MARKER}')) {{ throw ('native sandbox console probe failed: ' + (Get-Content -Raw -LiteralPath '{stderr}')) }};",
        powershell_literal(&executable),
    )
}

#[test]
#[ignore = "invoked by the sandbox console fixture"]
fn powershell_parent_has_no_console() {
    // Ordinary test runs are not required to have a console-free parent.
    if std::env::var(CONSOLE_PROBE_ENV).as_deref() != Ok("1")
        || std::env::args().nth(1).as_deref() != Some(CONSOLE_PROBE_TEST)
    {
        return;
    }

    // PowerShell may give this helper its own console. Detach only the helper
    // before checking the actual PowerShell parent's attachment. A parent with
    // no console is documented to produce ERROR_INVALID_HANDLE; all other
    // outcomes fail closed, including a successful attachment to a hidden one.
    assert_ne!(unsafe { FreeConsole() }, 0, "detach native console probe");
    let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    let error = unsafe { GetLastError() };
    assert_eq!(
        attached,
        0,
        "piped sandbox PowerShell unexpectedly has a console: {:?}",
        unsafe { GetConsoleWindow() },
    );
    assert_eq!(error, ERROR_INVALID_HANDLE, "probe parent console");
    assert!(unsafe { GetConsoleWindow() }.is_null());
    println!("{CONSOLE_PROBE_MARKER}");
}

// Keep the public regression's exact test name in tests.rs while its fixture
// and diagnostics live together instead of growing that large module.
pub(super) fn assert_capture_preserves_descendant() {
    let Some(pwsh) = pwsh_path() else {
        return;
    };
    let _guard = legacy_process_test_guard();
    let cwd = sandbox_cwd();
    // Descendant files belong in the writable workspace, not sandbox control state.
    let fixture = TempDir::new_in(&cwd).expect("create descendant lifecycle fixture");
    let codex_home = sandbox_home("legacy-capture-pwsh");
    println!("capture pwsh codex_home={}", codex_home.path().display());
    let ready_marker = fixture.path().join("descendant-started");
    let observed_marker = fixture.path().join("descendant-observed-alive");
    let release_marker = fixture.path().join("release-descendant");
    let survival_marker = fixture.path().join("descendant-survived");
    let expiry_marker = fixture.path().join("descendant-deadline-expired");
    let descendant_command = format!(
        "$deadline=(Get-Date).AddSeconds(30); Set-Content -LiteralPath '{}' -Value $PID; while (-not (Test-Path -LiteralPath '{}')) {{ if ((Get-Date) -ge $deadline) {{ Set-Content -LiteralPath '{}' -Value expired; exit 3 }}; Start-Sleep -Milliseconds 25 }}; Set-Content -LiteralPath '{}' -Value survived",
        powershell_literal(&ready_marker),
        powershell_literal(&release_marker),
        powershell_literal(&expiry_marker),
        powershell_literal(&survival_marker),
    );
    let parent_tail = format!(
        "while (-not (Test-Path -LiteralPath '{}')) {{ Start-Sleep -Milliseconds 25 }}",
        powershell_literal(&observed_marker),
    );
    let assert_no_console = powershell_no_console_probe(fixture.path());
    let parent_command = format!(
        "{assert_no_console} Write-Output LEGACY-CAPTURE-DIRECT; {}",
        start_powershell_child(&pwsh, fixture.path(), &descendant_command, &parent_tail),
    );
    // Keep the existing Start-Process launch and redirections. Redirected stdio
    // does not rule out extra inherited capture writers; this regression must
    // still expose capture waiting for those writers until the child expires.
    let capture_finished = AtomicBool::new(false);
    let started_at = Instant::now();
    let (result, capture_elapsed, descendant_observation) = std::thread::scope(|scope| {
        // Wake the observer on normal return, errors, and unwinding before the
        // scoped thread is joined. The capture's existing timeout stays in charge.
        let finished = CaptureFinished(&capture_finished);
        let observer = scope.spawn(|| {
            loop {
                if let Ok(pid) = fs::read_to_string(&ready_marker)
                    && let Ok(pid) = pid.trim().parse()
                {
                    let handle = unsafe {
                        OpenProcess(
                            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                            /*bInheritHandle*/ 0,
                            pid,
                        )
                    };
                    if handle.is_null() {
                        return Err(io::Error::last_os_error());
                    }
                    // SAFETY: OpenProcess returned a fresh owned handle. This
                    // observer transfers it to the test, keeping this process
                    // identity available even after exit and possible PID reuse.
                    let process = unsafe { OwnedHandle::from_raw_handle(handle) };
                    let state = unsafe {
                        WaitForSingleObject(process.as_raw_handle(), /*dwMilliseconds*/ 0)
                    };
                    if state != WAIT_TIMEOUT {
                        return Err(io::Error::other(format!(
                            "descendant was not alive when observed: wait={state}"
                        )));
                    }
                    // The root waits for this acknowledgement, so its normal
                    // exit cannot precede acquisition of the live child handle.
                    fs::write(&observed_marker, "observed alive")?;
                    return Ok((process, started_at.elapsed()));
                }
                if capture_finished.load(Ordering::Acquire) {
                    return Err(io::Error::other(
                        "capture returned before acquiring a live descendant handle",
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        let result = run_windows_sandbox_capture(
            &PermissionProfile::workspace_write(),
            workspace_roots_for(cwd.as_path()).as_slice(),
            codex_home.path(),
            vec![
                pwsh.display().to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                parent_command,
            ],
            cwd.as_path(),
            console_probe_env(),
            Some(10_000),
            /*cancellation*/ None,
        );
        let capture_elapsed = started_at.elapsed();
        drop(finished);
        let observation = observer.join().expect("join descendant observer");
        (result, capture_elapsed, observation)
    });
    let deadline_expired = expiry_marker.exists();
    let state_before_release = descendant_observation
        .as_ref()
        .map(|(process, observed_at)| {
            let wait = unsafe {
                WaitForSingleObject(process.as_raw_handle(), /*dwMilliseconds*/ 0)
            };
            let exit_code = (wait == WAIT_OBJECT_0).then(|| process_exit_code(process));
            (observed_at, wait, exit_code)
        });
    // Release before any assertions, including failures in the observations.
    fs::write(&release_marker, "release").expect("release descendant after root exit");
    let descendant_stderr = fs::read(fixture.path().join("descendant.stderr")).unwrap_or_default();
    let descendant_stderr =
        String::from_utf8_lossy(&descendant_stderr[..descendant_stderr.len().min(8 * 1024)]);
    let lifecycle_diagnostics = format!(
        "capture_elapsed={capture_elapsed:?} deadline_expired={deadline_expired} descendant_before_release(observed_alive_at,wait,exit_code)={state_before_release:?} descendant_stderr(first 8192 bytes)={descendant_stderr:?}"
    );
    let result = result.unwrap_or_else(|err| {
        panic!("run legacy capture powershell: {err:#}; {lifecycle_diagnostics}")
    });
    let diagnostics = format!(
        "{lifecycle_diagnostics}; {}",
        capture_result_diagnostics(&result)
    );
    println!("{diagnostics}");
    let (descendant_process, _) = descendant_observation
        .unwrap_or_else(|err| panic!("observe descendant while alive: {err}; {diagnostics}"));
    assert_eq!(result.exit_code, 0, "{diagnostics}");
    assert!(!result.timed_out, "{diagnostics}");
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("LEGACY-CAPTURE-DIRECT"),
        "{diagnostics}"
    );
    assert!(
        wait_for_path(&survival_marker, Duration::from_secs(10)),
        "sandbox descendant did not survive normal capture exit; {diagnostics}"
    );
    wait_for_process_exit(&descendant_process, Duration::from_secs(10)).unwrap_or_else(|err| {
        panic!("descendant did not exit after release: {err}; {diagnostics}")
    });
    let final_exit_code = process_exit_code(&descendant_process)
        .unwrap_or_else(|err| panic!("query final descendant exit: {err}; {diagnostics}"));
    println!("descendant_after_release(exit_code)={final_exit_code}");
}

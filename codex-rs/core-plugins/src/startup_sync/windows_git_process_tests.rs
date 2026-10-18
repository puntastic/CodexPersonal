use super::run_git_command_with_timeout;
use pretty_assertions::assert_eq;
use std::ffi::c_void;
use std::fs;
use std::io;
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

const HELPER_TEST: &str = "startup_sync::windows_git_process_tests::native_process_fixture";
const PHASE_ENV: &str = "CODEX_TEST_CURATED_GIT_PHASE";
const FIXTURE_ENV: &str = "CODEX_TEST_CURATED_GIT_FIXTURE";
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const SYNCHRONIZE_AND_TERMINATE: u32 = 0x0010_0001;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetConsoleWindow() -> *mut c_void;
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
    fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
    fn TerminateProcess(handle: *mut c_void, exit_code: u32) -> i32;
}

struct FixtureProcess(OwnedHandle);

impl FixtureProcess {
    fn open(process_id: u32) -> io::Result<Self> {
        // Retain the process identity before it can exit and its PID be reused.
        let handle = unsafe {
            OpenProcess(SYNCHRONIZE_AND_TERMINATE, /*inherit*/ 0, process_id)
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle) }))
    }

    fn assert_exited(&self) {
        assert_eq!(
            unsafe {
                WaitForSingleObject(self.0.as_raw_handle(), /*milliseconds*/ 5_000)
            },
            WAIT_OBJECT_0,
            "owned Git fixture process survived cleanup"
        );
    }
}

impl Drop for FixtureProcess {
    fn drop(&mut self) {
        // A failing regression must not leave its deliberately long-lived child.
        if unsafe {
            WaitForSingleObject(self.0.as_raw_handle(), /*milliseconds*/ 0)
        } == WAIT_TIMEOUT
        {
            unsafe {
                TerminateProcess(self.0.as_raw_handle(), /*exit_code*/ 1);
                WaitForSingleObject(self.0.as_raw_handle(), /*milliseconds*/ 5_000);
            }
        }
    }
}

#[allow(
    clippy::expect_used,
    reason = "Native fixture setup failures are test assertions."
)]
fn fixture_command(phase: &str, fixture: &Path) -> Command {
    let mut command = codex_utils_process::background_command(
        std::env::current_exe().expect("native fixture executable"),
    );
    command
        .args(["--exact", HELPER_TEST, "--ignored", "--nocapture"])
        .env(PHASE_ENV, phase)
        .env(FIXTURE_ENV, fixture);
    command
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(/*secs*/ 10);
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "native fixture did not become ready"
        );
        std::thread::sleep(Duration::from_millis(/*millis*/ 10));
    }
}

#[allow(
    clippy::expect_used,
    reason = "Native fixture setup failures are test assertions."
)]
fn capture_tree_processes(fixture: &Path) -> [FixtureProcess; 2] {
    wait_for_path(&fixture.join("tree-ready"));
    ["root-pid", "child-pid"].map(|name| {
        let path = fixture.join(name);
        wait_for_path(&path);
        let process_id = fs::read_to_string(path)
            .expect("read native fixture PID")
            .parse()
            .expect("parse native fixture PID");
        FixtureProcess::open(process_id).expect("retain native fixture process")
    })
}

#[test]
#[ignore = "native subprocess fixture; launched explicitly by behavioral tests"]
#[allow(
    clippy::zombie_processes,
    reason = "This Windows fixture deliberately leaves its child for the owned-job lifetime assertions; child holds are bounded."
)]
fn native_process_fixture() {
    let Ok(phase) = std::env::var(PHASE_ENV) else {
        return;
    };
    assert!(unsafe { GetConsoleWindow() }.is_null());
    let fixture =
        std::path::PathBuf::from(std::env::var_os(FIXTURE_ENV).expect("native fixture directory"));
    match phase.as_str() {
        "output" => {
            println!("curated Git stdout");
            eprintln!("curated Git stderr");
        }
        "tree" | "root-exit" => {
            // Spawn immediately: post-spawn job assignment would leave a race.
            let child = fixture_command("child", &fixture)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("spawn immediate Git descendant");
            fs::write(fixture.join("root-pid"), std::process::id().to_string())
                .expect("record root PID");
            fs::write(fixture.join("child-pid"), child.id().to_string()).expect("record child PID");
            wait_for_path(&fixture.join("child-ready"));
            fs::write(fixture.join("tree-ready"), b"ready").expect("publish completed PID files");
            if phase == "root-exit" {
                wait_for_path(&fixture.join("release-root-exit"));
                println!("curated Git root completed");
                io::stdout().flush().expect("flush root output");
                std::process::exit(23);
            }
            std::thread::sleep(Duration::from_secs(/*secs*/ 30));
        }
        "child" => {
            eprintln!("curated Git descendant stderr");
            fs::write(fixture.join("child-ready"), b"ready").expect("record child readiness");
            std::thread::sleep(Duration::from_secs(/*secs*/ 30));
        }
        "owner" => {
            let command = fixture_command("tree", &fixture);
            std::thread::spawn(move || {
                let _ = run_git_command_with_timeout(
                    command,
                    "owner-exit fixture",
                    Duration::from_secs(/*secs*/ 30),
                );
            });
            wait_for_path(&fixture.join("exit-owner"));
            // Exit without joining the background worker or running destructors.
            std::process::exit(0);
        }
        other => panic!("unknown native fixture phase: {other}"),
    }
}

#[test]
fn strict_startup_git_captures_output_without_a_console() {
    let fixture = tempfile::tempdir().expect("native fixture directory");
    let output = run_git_command_with_timeout(
        fixture_command("output", fixture.path()),
        "output fixture",
        Duration::from_secs(/*secs*/ 10),
    )
    .expect("run strict startup Git fixture");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("curated Git stdout"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("curated Git stderr"));
}

#[test]
fn strict_startup_git_timeout_terminates_immediate_descendants() {
    let fixture = tempfile::tempdir().expect("native fixture directory");
    let directory = fixture.path().to_owned();
    let observer = std::thread::spawn(move || capture_tree_processes(&directory));
    let started = Instant::now();
    let error = run_git_command_with_timeout(
        fixture_command("tree", fixture.path()),
        "timeout fixture",
        Duration::from_secs(/*secs*/ 5),
    )
    .expect_err("fixture must hit the original process deadline");
    let processes = observer.join().expect("capture native fixture processes");
    for process in &processes {
        process.assert_exited();
    }
    assert!(started.elapsed() < Duration::from_secs(/*secs*/ 15));
    assert!(error.contains("timeout fixture timed out after 5s"));
    assert!(error.contains("curated Git descendant stderr"));
}

#[test]
fn strict_startup_git_owner_exit_terminates_background_descendants() {
    let fixture = tempfile::tempdir().expect("native fixture directory");
    let owner = fixture_command("owner", fixture.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start background sync owner");
    let owner_process = FixtureProcess::open(owner.id()).expect("retain owner process");
    let processes = capture_tree_processes(fixture.path());
    fs::write(fixture.path().join("exit-owner"), b"exit").expect("release owner exit");
    owner_process.assert_exited();
    for process in &processes {
        process.assert_exited();
    }
    let output = owner.wait_with_output().expect("drain exited owner output");
    assert!(output.status.success());
}

#[test]
fn strict_startup_git_root_exit_preserves_status_and_output_without_waiting_for_child() {
    let fixture = tempfile::tempdir().expect("native fixture directory");
    let directory = fixture.path().to_owned();
    let observer = std::thread::spawn(move || {
        let processes = capture_tree_processes(&directory);
        fs::write(directory.join("release-root-exit"), b"exit").expect("release root exit");
        processes
    });
    let started = Instant::now();
    let output = run_git_command_with_timeout(
        fixture_command("root-exit", fixture.path()),
        "completed root fixture",
        Duration::from_secs(/*secs*/ 10),
    )
    .expect("root completion must not wait for the descendant hold deadline");
    let processes = observer.join().expect("capture native fixture processes");
    for process in &processes {
        process.assert_exited();
    }
    assert_eq!(output.status.code(), Some(23));
    assert!(started.elapsed() < Duration::from_secs(/*secs*/ 15));
    assert!(String::from_utf8_lossy(&output.stdout).contains("curated Git root completed"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("curated Git descendant stderr"));
}

#[test]
fn strict_startup_git_reports_launch_failure() {
    let fixture = tempfile::tempdir().expect("native fixture directory");
    let error = run_git_command_with_timeout(
        Command::new(fixture.path().join("missing-startup-git.exe")),
        "missing executable fixture",
        Duration::from_secs(/*secs*/ 1),
    )
    .expect_err("missing executable must report a launch error");
    assert!(error.contains("failed to run missing executable fixture"));
}

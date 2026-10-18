//! Bounded observations and failure cleanup for the legacy process fixtures.
//! These helpers do not change sandbox permissions or production process handling.

use super::powershell_literal;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use codex_utils_pty::JobObject;
use codex_utils_pty::ProcessHandle;
use codex_utils_pty::SpawnedProcess;
use std::fs::File;
use std::io;
use std::io::Read;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio::time::sleep_until;
use windows_sys::Win32::System::Threading::OpenProcess;
use windows_sys::Win32::System::Threading::PROCESS_SYNCHRONIZE;
use windows_sys::Win32::System::Threading::PROCESS_TERMINATE;

const OUTPUT_LIMIT: usize = 8 * 1024;
const FILE_LIMIT: u64 = 2 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const FAILURE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const FAILURE_RELEASE_GRACE: Duration = Duration::from_secs(1);

#[derive(Default, Debug, PartialEq, Eq)]
struct BoundedOutput {
    bytes: Vec<u8>,
    observed: usize,
}

impl BoundedOutput {
    fn append(&mut self, bytes: &[u8]) {
        self.observed = self.observed.saturating_add(bytes.len());
        let retained = bytes
            .len()
            .min(OUTPUT_LIMIT.saturating_sub(self.bytes.len()));
        self.bytes.extend_from_slice(&bytes[..retained]);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitObservation {
    Pending,
    Reported(i32),
    ChannelClosed,
}

pub(super) struct ObservedProcess {
    session: Option<ProcessHandle>,
    stdout_rx: mpsc::Receiver<Vec<u8>>,
    stderr_rx: mpsc::Receiver<Vec<u8>>,
    exit_rx: oneshot::Receiver<i32>,
    stdout: BoundedOutput,
    stderr: BoundedOutput,
    stdout_closed: bool,
    stderr_closed: bool,
    exit: ExitObservation,
}

impl ObservedProcess {
    pub(super) fn new(spawned: SpawnedProcess) -> Self {
        let SpawnedProcess {
            session,
            stdout_rx,
            stderr_rx,
            exit_rx,
        } = spawned;
        Self {
            session: Some(session),
            stdout_rx,
            stderr_rx,
            exit_rx,
            stdout: BoundedOutput::default(),
            stderr: BoundedOutput::default(),
            stdout_closed: false,
            stderr_closed: false,
            exit: ExitObservation::Pending,
        }
    }

    pub(super) fn request_terminate(&self) {
        if let Some(session) = &self.session {
            session.request_terminate();
        }
    }

    async fn observe_until(&mut self, deadline: Instant) {
        tokio::select! {
            _ = sleep_until(deadline) => {}
            chunk = self.stdout_rx.recv(), if !self.stdout_closed => {
                match chunk {
                    Some(chunk) => self.stdout.append(&chunk),
                    None => self.stdout_closed = true,
                }
            }
            chunk = self.stderr_rx.recv(), if !self.stderr_closed => {
                match chunk {
                    Some(chunk) => self.stderr.append(&chunk),
                    None => self.stderr_closed = true,
                }
            }
            result = &mut self.exit_rx, if self.exit == ExitObservation::Pending => {
                self.exit = match result {
                    Ok(code) => ExitObservation::Reported(code),
                    Err(_) => ExitObservation::ChannelClosed,
                };
            }
        }
    }

    pub(super) async fn wait_for_path(&mut self, path: &Path, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !path.exists() && Instant::now() < deadline {
            // Unlike the old blocking fixture poll, this lets the current-thread
            // runtime service output and exit forwarding while readiness is pending.
            self.observe_until(deadline.min(Instant::now() + POLL_INTERVAL))
                .await;
        }
        path.exists()
    }

    pub(super) async fn finish(&mut self, timeout: Duration) -> Result<i32, &'static str> {
        let exit_deadline = Instant::now() + timeout;
        while self.exit == ExitObservation::Pending && Instant::now() < exit_deadline {
            self.observe_until(exit_deadline).await;
        }
        if self.exit == ExitObservation::Pending {
            return Err("timed out waiting for exit");
        }
        // Preserve the fixture's separate exit and output-drain deadlines, but
        // service both streams throughout rather than discarding stderr.
        let output_deadline = Instant::now() + timeout;
        while !(self.stdout_closed && self.stderr_closed) && Instant::now() < output_deadline {
            self.observe_until(output_deadline).await;
        }
        if !(self.stdout_closed && self.stderr_closed) {
            return Err("timed out waiting for output EOF");
        }
        let exit_code = match self.exit {
            ExitObservation::Reported(code) => code,
            ExitObservation::ChannelClosed => -1,
            ExitObservation::Pending => unreachable!("exit observation checked above"),
        };
        // The old collector dropped the session before returning. Preserve that
        // boundary so a held session cannot hide kill-on-close regressions when
        // the caller releases the descendant only after collection completes.
        drop(self.session.take());
        Ok(exit_code)
    }

    pub(super) fn diagnostics(&self) -> String {
        format!(
            "exit_observation={:?} (Pending is not proof of OS liveness); stdout_closed={} stderr_closed={}\nstdout(first {}/{} bytes):\n{}\nstderr(first {}/{} bytes):\n{}",
            self.exit,
            self.stdout_closed,
            self.stderr_closed,
            self.stdout.bytes.len(),
            self.stdout.observed,
            String::from_utf8_lossy(&self.stdout.bytes),
            self.stderr.bytes.len(),
            self.stderr.observed,
            String::from_utf8_lossy(&self.stderr.bytes),
        )
    }

    pub(super) async fn cleanup(
        &mut self,
        release_marker: Option<&Path>,
        descendant: Option<&OwnedHandle>,
    ) -> String {
        // Cleanup is separate from the unchanged readiness/lifecycle deadlines.
        // Release first, then terminate only the process tree owned by this session.
        let release =
            release_marker.map(|path| std::fs::write(path, "release after fixture failure"));
        self.request_terminate();
        let started = Instant::now();
        let deadline = started + FAILURE_CLEANUP_TIMEOUT;
        let mut descendant_termination = None;
        loop {
            let descendant_state =
                descendant.map(|process| super::wait_for_process_exit(process, Duration::ZERO));
            if descendant_termination.is_none()
                && Instant::now() >= started + FAILURE_RELEASE_GRACE
                && descendant_state.as_ref().is_some_and(
                    |state| matches!(state, Err(error) if error.kind() == io::ErrorKind::TimedOut),
                )
                && let Some(process) = descendant
            {
                // Preserved jobs deliberately ignore later terminate requests.
                // This handle was acquired for this fixture child before release;
                // never re-open or terminate a process by a possibly reused PID.
                descendant_termination = Some(JobObject::terminate_process_handle(process));
            }
            let output_finished = self.stdout_closed && self.stderr_closed;
            if (self.exit != ExitObservation::Pending
                && output_finished
                && descendant_state.as_ref().is_none_or(Result::is_ok))
                || Instant::now() >= deadline
            {
                return format!(
                    "failure_cleanup: release={release:?} descendant_termination={descendant_termination:?} descendant_wait={descendant_state:?}\n{}",
                    self.diagnostics(),
                );
            }
            self.observe_until(deadline.min(Instant::now() + POLL_INTERVAL))
                .await;
        }
    }
}

pub(super) fn open_descendant_for_cleanup(pid: u32) -> io::Result<OwnedHandle> {
    let handle = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
            /*bInheritHandle*/ 0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcess returned a fresh handle with wait and terminate access.
    // Retaining one owner preserves this process identity through failure cleanup.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

pub(super) async fn wait_for_process_exit(
    process: &OwnedHandle,
    timeout: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match super::wait_for_process_exit(process, Duration::ZERO) {
            Err(error) if error.kind() == io::ErrorKind::TimedOut && Instant::now() < deadline => {
                sleep_until(deadline.min(Instant::now() + POLL_INTERVAL)).await;
            }
            result => return result,
        }
    }
}

pub(super) fn file_diagnostics(path: &Path) -> String {
    let read = || -> io::Result<(u64, Vec<u8>)> {
        let file = File::open(path)?;
        let total = file.metadata()?.len();
        let mut bytes = Vec::new();
        file.take(FILE_LIMIT).read_to_end(&mut bytes)?;
        Ok((total, bytes))
    };
    match read() {
        Ok((total, bytes)) => format!(
            "{} (first {}/{total} bytes):\n{}",
            path.display(),
            bytes.len(),
            String::from_utf8_lossy(&bytes),
        ),
        Err(error) => format!("{}: {error}", path.display()),
    }
}

pub(super) fn tty_diagnostics(fixture: &Path, codex_home: &Path) -> String {
    [
        fixture.join("descendant-started"),
        fixture.join("descendant-survived"),
        fixture.join("descendant.stdout"),
        fixture.join("descendant.stderr"),
        crate::current_log_file_path(&codex_home.join(".sandbox")),
    ]
    .iter()
    .map(|path| file_diagnostics(path))
    .collect::<Vec<_>>()
    .join("\n")
}

pub(super) fn tty_parent_command(
    pwsh: &Path,
    fixture: &Path,
    child_command: &str,
    parent_tail: &str,
) -> String {
    let child_command = format!(
        "Write-Output LEGACY-TTY-PHASE-child-entered; try {{ {child_command} }} catch {{ Write-Error -ErrorRecord $_; exit 92 }}"
    );
    let encoded = BASE64.encode(
        child_command
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    format!(
        "Write-Output LEGACY-TTY-PHASE-parent-entered; try {{ $child = Start-Process -ErrorAction Stop -PassThru -WindowStyle Hidden -FilePath '{}' -ArgumentList '-NoProfile','-EncodedCommand','{encoded}' -RedirectStandardOutput '{}' -RedirectStandardError '{}'; Write-Output ('LEGACY-TTY-PHASE-child-launched pid=' + $child.Id); {parent_tail} }} catch {{ Write-Output LEGACY-TTY-PHASE-parent-failed; Write-Error -ErrorRecord $_; exit 91 }}",
        powershell_literal(pwsh),
        powershell_literal(&fixture.join("descendant.stdout")),
        powershell_literal(&fixture.join("descendant.stderr")),
    )
}

#[test]
fn bounded_output_retains_prefix_and_counts_discarded_bytes() {
    let mut output = BoundedOutput::default();
    output.append(&vec![b'a'; OUTPUT_LIMIT - 1]);
    output.append(b"bcd");
    output.append(b"ef");
    let mut expected = vec![b'a'; OUTPUT_LIMIT - 1];
    expected.push(b'b');
    pretty_assertions::assert_eq!(
        output,
        BoundedOutput {
            bytes: expected,
            observed: OUTPUT_LIMIT + 4,
        },
    );
}

#[test]
fn bounded_output_accepts_empty_chunks() {
    let mut output = BoundedOutput::default();
    output.append(&[]);
    output.append(b"diagnostic");
    output.append(&[]);
    pretty_assertions::assert_eq!(
        output,
        BoundedOutput {
            bytes: b"diagnostic".to_vec(),
            observed: b"diagnostic".len(),
        },
    );
}

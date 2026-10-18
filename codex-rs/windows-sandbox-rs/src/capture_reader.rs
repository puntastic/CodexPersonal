//! Capture-only pipe ownership and cooperative post-root output drainage.

use crate::WindowsSandboxCancellationToken;
use codex_protocol::exec_output::OutputCaptureEnd;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::os::windows::io::OwnedHandle;
use std::ptr;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;
use windows_sys::Win32::Foundation::ERROR_BROKEN_PIPE;
use windows_sys::Win32::Foundation::ERROR_NO_DATA;
use windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT;
use windows_sys::Win32::Foundation::SetHandleInformation;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Pipes::PIPE_NOWAIT;
use windows_sys::Win32::System::Pipes::SetNamedPipeHandleState;

const POLL_INTERVAL: Duration = Duration::from_millis(/*millis*/ 10);

pub(crate) struct CaptureStdio {
    pub(crate) stdin_read: OwnedHandle,
    pub(crate) stdin_write: OwnedHandle,
    pub(crate) stdout_read: NonblockingReadPipe,
    pub(crate) stdout_write: OwnedHandle,
    pub(crate) stderr_read: NonblockingReadPipe,
    pub(crate) stderr_write: OwnedHandle,
}

impl CaptureStdio {
    pub(crate) fn new() -> io::Result<Self> {
        let (stdin_read, stdin_write) = pipe_pair()?;
        let (stdout_read, stdout_write) = pipe_pair()?;
        let (stderr_read, stderr_write) = pipe_pair()?;
        for handle in [&stdin_read, &stdout_write, &stderr_write] {
            if unsafe {
                SetHandleInformation(
                    handle.as_raw_handle(),
                    HANDLE_FLAG_INHERIT,
                    HANDLE_FLAG_INHERIT,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self {
            stdin_read,
            stdin_write,
            // Only the parent's read ends become nonblocking. Child writers
            // retain their original blocking/backpressure and inheritance modes.
            stdout_read: NonblockingReadPipe::new(stdout_read)?,
            stdout_write,
            stderr_read: NonblockingReadPipe::new(stderr_read)?,
            stderr_write,
        })
    }
}

fn pipe_pair() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    if unsafe {
        CreatePipe(&mut read, &mut write, ptr::null_mut(), /*nsize*/ 0)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: A successful CreatePipe returns two distinct valid owned handles.
    Ok(unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    })
}

#[derive(Debug)]
pub(crate) struct NonblockingReadPipe(OwnedHandle);

impl NonblockingReadPipe {
    fn new(handle: OwnedHandle) -> io::Result<Self> {
        let mode = PIPE_NOWAIT;
        if unsafe {
            SetNamedPipeHandleState(handle.as_raw_handle(), &mode, ptr::null(), ptr::null())
        } == 0
        {
            // Fail before child creation; never retry with a blocking read end.
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }

    fn read_available(&self, buffer: &mut [u8]) -> io::Result<ReadChunk> {
        let mut count = 0;
        let ok = unsafe {
            ReadFile(
                self.0.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut count,
                ptr::null_mut(),
            )
        };
        if ok != 0 {
            // A successful zero-byte read can follow a zero-length write; it
            // does not establish that every writer has closed.
            return Ok(if count == 0 {
                ReadChunk::Idle
            } else {
                ReadChunk::Data(count as usize)
            });
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_NO_DATA) => Ok(ReadChunk::Idle),
            Some(ERROR_BROKEN_PIPE) => Ok(ReadChunk::Eof),
            _ => Err(error),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CapturedStream {
    pub(crate) bytes: Vec<u8>,
    pub(crate) end: OutputCaptureEnd,
}

enum ReadChunk {
    Data(usize),
    Idle,
    Eof,
}

enum ReadControl {
    FinishBy(Instant),
    Cancel,
}

fn apply_control(control: ReadControl, deadline: &mut Option<Instant>) -> Option<OutputCaptureEnd> {
    match control {
        ReadControl::FinishBy(next) => {
            *deadline = Some(deadline.map_or(next, |current| current.min(next)));
            None
        }
        ReadControl::Cancel => Some(OutputCaptureEnd::Cancelled),
    }
}

fn collect_output(
    mut read: impl FnMut(&mut [u8]) -> io::Result<ReadChunk>,
    control: mpsc::Receiver<ReadControl>,
) -> CapturedStream {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    let mut deadline = None;
    let end = 'reading: loop {
        // Check control for every chunk, not only when an active writer goes idle.
        loop {
            match control.try_recv() {
                Ok(command) => {
                    if let Some(end) = apply_control(command, &mut deadline) {
                        break 'reading end;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    break 'reading OutputCaptureEnd::Cancelled;
                }
            }
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            break OutputCaptureEnd::DrainDeadline;
        }
        match read(&mut buffer) {
            Ok(ReadChunk::Data(count)) => bytes.extend_from_slice(&buffer[..count]),
            Ok(ReadChunk::Eof) => break OutputCaptureEnd::Eof,
            Err(error) => {
                break OutputCaptureEnd::ReadError {
                    os_error: error.raw_os_error(),
                };
            }
            Ok(ReadChunk::Idle) => {
                let wait = deadline.map_or(POLL_INTERVAL, |deadline| {
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(POLL_INTERVAL)
                });
                match control.recv_timeout(wait) {
                    Ok(command) => {
                        if let Some(end) = apply_control(command, &mut deadline) {
                            break end;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break OutputCaptureEnd::Cancelled,
                }
            }
        }
    };
    CapturedStream { bytes, end }
}

struct CaptureReader {
    control: mpsc::Sender<ReadControl>,
    thread: Option<JoinHandle<CapturedStream>>,
}

impl CaptureReader {
    fn start(pipe: NonblockingReadPipe, name: &str) -> io::Result<Self> {
        let (control, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || collect_output(|buffer| pipe.read_available(buffer), receiver))?;
        Ok(Self {
            control,
            thread: Some(thread),
        })
    }

    fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn join(&mut self) -> io::Result<CapturedStream> {
        self.thread
            .take()
            .expect("capture reader is joined once")
            .join()
            .map_err(|_| io::Error::other("capture output reader panicked"))
    }
}

impl Drop for CaptureReader {
    fn drop(&mut self) {
        let _ = self.control.send(ReadControl::Cancel);
        if let Some(thread) = self.thread.take() {
            // The owned pipe is nonblocking, so even an early-return path can
            // join this reader without depending on a descendant's lifetime.
            let _ = thread.join();
        }
    }
}

pub(crate) struct CaptureOutput {
    stdout: CaptureReader,
    stderr: CaptureReader,
}

impl CaptureOutput {
    pub(crate) fn start(
        stdout: NonblockingReadPipe,
        stderr: NonblockingReadPipe,
    ) -> io::Result<Self> {
        let stdout = CaptureReader::start(stdout, "sandbox-capture-stdout")?;
        let stderr = CaptureReader::start(stderr, "sandbox-capture-stderr")?;
        Ok(Self { stdout, stderr })
    }

    pub(crate) fn finish(
        mut self,
        deadline: Instant,
        cancellation: Option<&WindowsSandboxCancellationToken>,
    ) -> io::Result<(CapturedStream, CapturedStream)> {
        // Both readers receive the same absolute deadline before either is joined.
        let _ = self.stdout.control.send(ReadControl::FinishBy(deadline));
        let _ = self.stderr.control.send(ReadControl::FinishBy(deadline));
        let mut cancelled = false;
        while !self.stdout.is_finished() || !self.stderr.is_finished() {
            if !cancelled && cancellation.is_some_and(WindowsSandboxCancellationToken::is_cancelled)
            {
                cancelled = true;
                let _ = self.stdout.control.send(ReadControl::Cancel);
                let _ = self.stderr.control.send(ReadControl::Cancel);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        let stdout = self.stdout.join();
        let stderr = self.stderr.join();
        Ok((stdout?, stderr?))
    }
}

#[cfg(test)]
#[path = "capture_reader_tests.rs"]
mod tests;

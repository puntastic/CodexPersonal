use super::*;
use pretty_assertions::assert_eq;
use std::fs::File;
use std::io::Write;
use windows_sys::Win32::System::Threading::CreateEventW;

fn capture_with_writers() -> (CaptureOutput, File, File) {
    let pipes = CaptureStdio::new().expect("create capture stdio");
    drop(pipes.stdin_read);
    drop(pipes.stdin_write);
    let capture =
        CaptureOutput::start(pipes.stdout_read, pipes.stderr_read).expect("start readers");
    (
        capture,
        File::from(pipes.stdout_write),
        File::from(pipes.stderr_write),
    )
}

#[test]
fn empty_connected_pipe_is_not_eof_and_partial_reads_preserve_bytes() {
    let (read, write) = pipe_pair().expect("create capture pipe");
    let read = NonblockingReadPipe::new(read).expect("make read end nonblocking");
    let mut write = File::from(write);
    let mut buffer = [0; 16];
    assert!(matches!(
        read.read_available(&mut buffer).expect("empty read"),
        ReadChunk::Idle
    ));
    write
        .write_all(b"ABCDEFGHI")
        .expect("write available bytes");
    assert!(matches!(
        read.read_available(&mut buffer[..4]).expect("partial read"),
        ReadChunk::Data(4)
    ));
    assert_eq!(&buffer[..4], b"ABCD");
    assert!(matches!(
        read.read_available(&mut buffer).expect("remaining read"),
        ReadChunk::Data(5)
    ));
    assert_eq!(&buffer[..5], b"EFGHI");
    assert!(matches!(
        read.read_available(&mut buffer).expect("empty after drain"),
        ReadChunk::Idle
    ));
    drop(write);
    assert!(matches!(
        read.read_available(&mut buffer).expect("closed writer"),
        ReadChunk::Eof
    ));
}

#[test]
fn non_pipe_handle_is_rejected_instead_of_starting_a_blocking_reader() {
    let event = unsafe {
        CreateEventW(
            ptr::null(),
            /*bmanualreset*/ 0,
            /*binitialstate*/ 0,
            ptr::null(),
        )
    };
    assert!(!event.is_null(), "{}", io::Error::last_os_error());
    // SAFETY: CreateEventW returned a fresh valid handle owned by this test.
    let event = unsafe { OwnedHandle::from_raw_handle(event) };
    let error = NonblockingReadPipe::new(event).expect_err("mode setting must fail closed");
    assert!(error.raw_os_error().is_some());
}

#[test]
fn capture_reaches_eof_with_exact_large_output_on_both_streams() {
    let (capture, mut stdout, mut stderr) = capture_with_writers();
    let stdout_bytes = (0..1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let stderr_bytes = vec![b'e'; 65_537];
    let expected_stdout = stdout_bytes.clone();
    let expected_stderr = stderr_bytes.clone();
    let stdout = std::thread::spawn(move || stdout.write_all(&stdout_bytes));
    let stderr = std::thread::spawn(move || stderr.write_all(&stderr_bytes));
    // Establish the drain bound before joining producers, so a failed reader
    // cannot leave the test waiting indefinitely for a full pipe's writer.
    let output = capture.finish(
        Instant::now() + Duration::from_secs(/*secs*/ 2),
        /*cancellation*/ None,
    );
    stdout
        .join()
        .expect("join stdout writer")
        .expect("write stdout");
    stderr
        .join()
        .expect("join stderr writer")
        .expect("write stderr");
    assert_eq!(
        output.expect("finish capture"),
        (
            CapturedStream {
                bytes: expected_stdout,
                end: OutputCaptureEnd::Eof
            },
            CapturedStream {
                bytes: expected_stderr,
                end: OutputCaptureEnd::Eof
            },
        )
    );
}

#[test]
fn retained_idle_writers_share_one_absolute_deadline() {
    let (capture, _stdout, _stderr) = capture_with_writers();
    let started = Instant::now();
    let timeout = Duration::from_millis(/*millis*/ 300);
    let (stdout, stderr) = capture
        .finish(started + timeout, /*cancellation*/ None)
        .expect("finish capture");
    assert_eq!(
        (stdout, stderr),
        (
            CapturedStream {
                bytes: Vec::new(),
                end: OutputCaptureEnd::DrainDeadline
            },
            CapturedStream {
                bytes: Vec::new(),
                end: OutputCaptureEnd::DrainDeadline
            },
        )
    );
    assert!(started.elapsed() >= timeout);
    assert!(started.elapsed() < timeout + Duration::from_millis(/*millis*/ 200));
}

#[test]
fn continuing_output_does_not_extend_the_drain_deadline() {
    let (capture, mut writer, _stderr) = capture_with_writers();
    const CHUNK: &[u8] = b"continuing output\n";
    let (ready_tx, ready_rx) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        let stop = Instant::now() + Duration::from_secs(/*secs*/ 2);
        writer.write_all(CHUNK).expect("write first chunk");
        ready_tx.send(()).expect("signal first write");
        while Instant::now() < stop {
            if writer.write_all(CHUNK).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(/*millis*/ 1));
        }
    });
    ready_rx
        .recv_timeout(Duration::from_secs(/*secs*/ 1))
        .expect("writer ready");
    let started = Instant::now();
    let output = capture.finish(
        started + Duration::from_millis(/*millis*/ 150),
        /*cancellation*/ None,
    );
    writer.join().expect("join bounded writer");
    let (stdout, stderr) = output.expect("finish capture");
    assert_eq!(
        (stdout.end, stderr.end),
        (
            OutputCaptureEnd::DrainDeadline,
            OutputCaptureEnd::DrainDeadline
        )
    );
    assert!(!stdout.bytes.is_empty());
    assert!(
        stdout
            .bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == CHUNK[index % CHUNK.len()])
    );
    assert!(started.elapsed() < Duration::from_secs(/*secs*/ 1));
}

#[test]
fn cancellation_stops_collectors_while_writers_remain_owned() {
    let (capture, mut stdout_writer, mut stderr_writer) = capture_with_writers();
    let cancellation = WindowsSandboxCancellationToken::new(|| true);
    let started = Instant::now();
    let (stdout, stderr) = capture
        .finish(
            started + Duration::from_secs(/*secs*/ 2),
            Some(&cancellation),
        )
        .expect("cancel capture");
    assert_eq!(
        (stdout.end, stderr.end),
        (OutputCaptureEnd::Cancelled, OutputCaptureEnd::Cancelled)
    );
    // The caller still owns both write handles; closing the collectors means
    // subsequent writes can fail, not that a writing descendant was terminated.
    assert!(stdout_writer.write_all(b"later").is_err());
    assert!(stderr_writer.write_all(b"later").is_err());
    assert!(started.elapsed() < Duration::from_secs(/*secs*/ 1));
}

#[test]
fn already_collected_bytes_survive_read_error_and_cancellation() {
    for cancel in [false, true] {
        let (control, receiver) = mpsc::channel();
        let mut reads = 0;
        let output = collect_output(
            |buffer| {
                reads += 1;
                if reads == 1 {
                    buffer[..6].copy_from_slice(b"prefix");
                    if cancel {
                        control
                            .send(ReadControl::Cancel)
                            .expect("request cancellation");
                    }
                    Ok(ReadChunk::Data(6))
                } else {
                    Err(io::Error::from_raw_os_error(/*code*/ 123))
                }
            },
            receiver,
        );
        assert_eq!(
            output,
            CapturedStream {
                bytes: b"prefix".to_vec(),
                end: if cancel {
                    OutputCaptureEnd::Cancelled
                } else {
                    OutputCaptureEnd::ReadError {
                        os_error: Some(123),
                    }
                },
            }
        );
    }
}

#[test]
fn dropping_capture_joins_readers_even_with_open_writers() {
    let (capture, mut stdout, mut stderr) = capture_with_writers();
    let started = Instant::now();
    drop(capture);
    assert!(stdout.write_all(b"later").is_err());
    assert!(stderr.write_all(b"later").is_err());
    assert!(started.elapsed() < Duration::from_secs(/*secs*/ 1));
}

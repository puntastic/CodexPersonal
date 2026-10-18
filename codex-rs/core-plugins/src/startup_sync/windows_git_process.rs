//! Strict tree ownership for automatic curated-plugin Git work on Windows.
//! The process-scoped startup worker remains asynchronous; its Git descendants
//! cannot outlive their owning job, including when the host process exits.

use codex_utils_pty::JobObject;
use std::io;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Output;

pub(super) struct StartupGitProcess {
    // Drop the job before the child and runtime on early-return paths.
    job: JobObject,
    child: tokio::process::Child,
    runtime: tokio::runtime::Runtime,
}

impl StartupGitProcess {
    pub(super) fn spawn(command: Command) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let job = JobObject::create_without_breakaway()?;
        let mut command = tokio::process::Command::from(command);
        let child = {
            let _entered = runtime.enter();
            // Assignment precedes execution. Unlike spawn_background, failure
            // must not retry with an uncontained process tree.
            job.spawn_contained(&mut command)?
        };
        Ok(Self {
            job,
            child,
            runtime,
        })
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub(super) fn kill(&mut self) -> io::Result<()> {
        self.job.terminate()
    }

    pub(super) fn wait_with_output(self) -> io::Result<Output> {
        let Self {
            job,
            child,
            runtime,
        } = self;
        // The caller has observed root exit or reached its original deadline.
        // Its observed root status is cached by Child::try_wait. Close all owned
        // descendant writers before joining output, retaining bytes already in
        // the pipes without waiting for an independent helper lifetime.
        job.terminate()?;
        let result = runtime.block_on(child.wait_with_output());
        // Automatic Git work owns its helpers; successful completion does not
        // grant descendants an independent lifetime.
        drop(job);
        result
    }
}

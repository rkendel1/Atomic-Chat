//! Runs one external process for the bench: no shell, stdin closed, output
//! captured within a size limit, stopped on timeout or cancellation.
//!
//! On Unix the process leads its own process group, so stopping it also stops
//! what it started (a test run, a compiler). It is first asked to stop with
//! SIGTERM, which stack-agent treats like Ctrl-C (stop at the next boundary
//! and report), and killed if it is still running after a grace period.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use super::run::CapturedText;

/// Captured bytes kept per stream; the middle of longer output is dropped.
pub const STDOUT_LIMIT: usize = 1024 * 1024;
pub const STDERR_LIMIT: usize = 256 * 1024;
/// How long a process asked to stop may take before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct Spec<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: &'a Path,
    /// Added to the inherited environment.
    pub env: &'a [(String, String)],
    /// Removed from the inherited environment.
    pub env_remove: &'a [String],
    pub timeout: Duration,
    pub stdout_limit: usize,
    pub stderr_limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Exited,
    TimedOut,
    Cancelled,
}

#[derive(Debug)]
pub struct Finished {
    pub ending: Ending,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub duration: Duration,
    pub stdout: Vec<u8>,
    pub stdout_total: u64,
    pub stderr: Vec<u8>,
    pub stderr_total: u64,
}

/// Starts the process and waits for it. `Err` only when it could not start.
pub async fn run(spec: Spec<'_>, cancel: &CancellationToken) -> Result<Finished, String> {
    let mut cmd = tokio::process::Command::new(spec.program);
    cmd.args(spec.args)
        .current_dir(spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::core::process_env::sanitize_tokio_command(&mut cmd);
    for name in spec.env_remove {
        cmd.env_remove(name);
    }
    for (k, v) in spec.env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", spec.program.display()))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let out_task = tokio::spawn(read_capped(stdout, spec.stdout_limit));
    let err_task = tokio::spawn(read_capped(stderr, spec.stderr_limit));

    let ending = tokio::select! {
        _ = child.wait() => Ending::Exited,
        _ = tokio::time::sleep(spec.timeout) => Ending::TimedOut,
        _ = cancel.cancelled() => Ending::Cancelled,
    };
    if ending != Ending::Exited {
        stop(&mut child).await;
    }
    let status = child.wait().await.ok();
    let duration = started.elapsed();
    let (stdout, stdout_total) = out_task.await.unwrap_or_default();
    let (stderr, stderr_total) = err_task.await.unwrap_or_default();

    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.and_then(|s| s.signal())
    };
    #[cfg(not(unix))]
    let signal = None;

    Ok(Finished {
        ending,
        exit_code: status.and_then(|s| s.code()),
        signal,
        duration,
        stdout,
        stdout_total,
        stderr,
        stderr_total,
    })
}

async fn stop(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;
        let group = Pid::from_raw(pid as i32);
        let _ = killpg(group, Signal::SIGTERM);
        if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_ok() {
            // The leader is gone; make sure nothing it started outlives it.
            let _ = killpg(group, Signal::SIGKILL);
            return;
        }
        let _ = killpg(group, Signal::SIGKILL);
    }
    let _ = child.start_kill();
}

/// Reads a stream to its end, keeping at most `limit` bytes: the first half
/// and the most recent half. Returns the kept bytes and the total read.
async fn read_capped<R: AsyncRead + Unpin>(mut reader: R, limit: usize) -> (Vec<u8>, u64) {
    let half = limit / 2;
    let mut head: Vec<u8> = Vec::new();
    let mut tail: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
    let mut total: u64 = 0;
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        total += n as u64;
        let mut chunk = &buf[..n];
        if head.len() < half {
            let take = (half - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
            chunk = &chunk[take..];
        }
        tail.extend(chunk);
        while tail.len() > limit - half {
            tail.pop_front();
        }
    }
    head.extend(tail);
    (head, total)
}

/// Turns captured bytes into text, marking where output was dropped.
pub fn captured(bytes: &[u8], total: u64, limit: usize) -> CapturedText {
    let truncated = total > bytes.len() as u64;
    let text = if truncated {
        let half = (limit / 2).min(bytes.len());
        format!(
            "{}\n[... {} bytes omitted ...]\n{}",
            String::from_utf8_lossy(&bytes[..half]),
            total - bytes.len() as u64,
            String::from_utf8_lossy(&bytes[half..])
        )
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    CapturedText {
        text,
        total_bytes: total,
        truncated,
    }
}

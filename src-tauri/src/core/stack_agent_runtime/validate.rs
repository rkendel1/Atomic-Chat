//! Health checks a runtime must pass before it is published or activated:
//! it launches and reports a version, its offline self-test passes (no model,
//! no network), and it offers the `work` interface Atomic Chat drives.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Flags of `stack-agent work` that Atomic Chat and Agent Bench pass.
pub const REQUIRED_WORK_FLAGS: &[&str] = &[
    "--kind",
    "--json",
    "--deadline-secs",
    "--max-turns",
    "--max-executions",
    "--print-reply",
    "--state-db",
];
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `bin args` in `cwd` with its home there too, so a check never reads
/// or writes the user's stack-agent state, and without model settings or
/// keys from Atomic Chat's environment.
pub fn run(bin: &Path, args: &[&str], cwd: &Path, timeout: Duration) -> Result<Output, String> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .current_dir(cwd)
        .env("HOME", cwd)
        .env("USERPROFILE", cwd)
        .env("XDG_DATA_HOME", cwd.join("data"))
        .env("XDG_STATE_HOME", cwd.join("state"))
        .env("XDG_CONFIG_HOME", cwd.join("config"));
    for (k, _) in std::env::vars_os() {
        if let Some(k) = k.to_str() {
            if k.starts_with("CHIP_") || k.ends_with("_API_KEY") || k.ends_with("_TOKEN") {
                cmd.env_remove(k);
            }
        }
    }
    crate::core::process_env::sanitize_std_command(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    run_command(cmd, timeout)
}

/// Runs a prepared command within `timeout`, capturing up to 1 MiB of each
/// output stream.
pub fn run_command(mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn_retrying_busy(&mut cmd)?;
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let out_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out.by_ref().take(1 << 20).read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let err_t = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = err.by_ref().take(1 << 20).read_to_end(&mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("did not finish within {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok(Output {
        code: status.code(),
        stdout: out_t.join().unwrap_or_default(),
        stderr: err_t.join().unwrap_or_default(),
    })
}

/// Spawns, retrying briefly on "text file busy" (ETXTBSY): an executable
/// just written by this process cannot be started while another thread's
/// fork still holds the write handle it inherited.
fn spawn_retrying_busy(cmd: &mut Command) -> Result<std::process::Child, String> {
    let mut attempt = 0;
    loop {
        match cmd.spawn() {
            Ok(c) => return Ok(c),
            Err(e) if e.raw_os_error() == Some(26) && attempt < 20 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("could not start: {e}")),
        }
    }
}

/// `stack-agent 0.1.0` → `0.1.0`.
pub fn parse_version(stdout: &str) -> Option<String> {
    let line = stdout.lines().next()?.trim();
    let v = line.strip_prefix("stack-agent ")?.trim();
    let mut parts = v.split(['-', '+']).next()?.split('.');
    let numeric = (0..3).all(|_| {
        parts
            .next()
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    });
    (numeric && parts.next().is_none()).then(|| v.to_string())
}

fn first_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

fn check(checks: &mut Vec<HealthCheck>, name: &str, passed: bool, detail: String) -> bool {
    checks.push(HealthCheck {
        name: name.to_string(),
        passed,
        detail,
    });
    passed
}

/// Runs every check; the version is returned only when all of them passed.
pub fn validate(bin: &Path, scratch: &Path) -> (Result<String, String>, Vec<HealthCheck>) {
    let mut checks = Vec::new();
    if let Err(e) = std::fs::create_dir_all(scratch) {
        return (
            Err(format!("could not create {}: {e}", scratch.display())),
            checks,
        );
    }

    let version = match run(bin, &["--version"], scratch, CHECK_TIMEOUT) {
        Ok(o) if o.code == Some(0) => parse_version(&o.stdout),
        _ => None,
    };
    let launched = check(
        &mut checks,
        "launches",
        version.is_some(),
        match &version {
            Some(v) => format!("`--version` reported stack-agent {v}"),
            None => "`--version` did not exit 0 with `stack-agent X.Y.Z`".to_string(),
        },
    );
    if !launched {
        let detail = checks.last().map(|c| c.detail.clone()).unwrap_or_default();
        return (
            Err(format!("health checks failed: launches ({detail})")),
            checks,
        );
    }

    let self_test = run(bin, &["--test"], scratch, CHECK_TIMEOUT);
    let (ok, detail) = match &self_test {
        Ok(o) if o.code == Some(0) && o.stdout.contains("Turn completed") => {
            (true, "offline self-test completed a turn".to_string())
        }
        Ok(o) => (
            false,
            format!(
                "`--test` exited {:?}: {}",
                o.code,
                first_line(if o.stderr.trim().is_empty() {
                    &o.stdout
                } else {
                    &o.stderr
                })
            ),
        ),
        Err(e) => (false, format!("`--test` {e}")),
    };
    check(&mut checks, "self_test", ok, detail);

    let help = run(bin, &["work", "--help"], scratch, CHECK_TIMEOUT);
    let (ok, detail) = match &help {
        Ok(o) => {
            let text = format!("{}{}", o.stdout, o.stderr);
            let missing: Vec<&str> = REQUIRED_WORK_FLAGS
                .iter()
                .copied()
                .filter(|f| !text.contains(f))
                .collect();
            if missing.is_empty() {
                (
                    true,
                    format!("`work` offers {}", REQUIRED_WORK_FLAGS.join(" ")),
                )
            } else {
                (false, format!("`work --help` lacks {}", missing.join(" ")))
            }
        }
        Err(e) => (false, format!("`work --help` {e}")),
    };
    check(&mut checks, "work_interface", ok, detail);

    let failed: Vec<String> = checks
        .iter()
        .filter(|c| !c.passed)
        .map(|c| format!("{} ({})", c.name, c.detail))
        .collect();
    if failed.is_empty() {
        (Ok(version.expect("launched")), checks)
    } else {
        (
            Err(format!("health checks failed: {}", failed.join("; "))),
            checks,
        )
    }
}

//! Where runtimes come from. [`GitSource`] resolves `main` with
//! `git ls-remote`, fetches exactly one commit and builds it with the
//! repository's own pinned toolchain (`rust-toolchain.toml`) and lockfile.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{is_full_commit, BIN_NAME, TRACKED_REF};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareError {
    /// `fetch`, `toolchain`, `build`, `validate` or `publish`.
    pub stage: String,
    pub message: String,
}

impl PrepareError {
    pub fn new(stage: &str, message: impl Into<String>) -> Self {
        Self {
            stage: stage.to_string(),
            message: message.into(),
        }
    }
}

pub struct Prepared {
    /// The built executable, inside the staging directory or the build cache.
    pub binary: PathBuf,
    pub toolchain: Option<String>,
}

pub trait Source: Send + Sync {
    fn repository(&self) -> &str;
    /// The commit `main` is at now.
    fn latest(&self) -> Result<String, String>;
    /// Up to `n` first-parent ancestors of `commit`, newest first: the
    /// earlier states of `main`, for when its tip cannot be prepared.
    fn history(&self, commit: &str, n: usize, scratch: &Path) -> Result<Vec<String>, String>;
    /// Produces an executable for exactly `commit`, working in `staging`.
    fn prepare(
        &self,
        commit: &str,
        staging: &Path,
        log: &mut dyn FnMut(&str),
    ) -> Result<Prepared, PrepareError>;
}

const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(30);
const FETCH_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const BUILD_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);

pub struct GitSource {
    pub repository: String,
    /// Cargo's target directory, shared by builds of every revision so a new
    /// revision rebuilds only what changed.
    pub build_dir: PathBuf,
    /// PATH for git and cargo: the user's login-shell PATH.
    pub path_env: String,
}

impl GitSource {
    fn find(&self, bin: &str) -> Option<PathBuf> {
        crate::core::agent_bench::adapters::search_path(bin, &self.path_env)
    }

    fn command(&self, program: &Path, cwd: &Path) -> Command {
        let mut c = Command::new(program);
        c.current_dir(cwd)
            .env("PATH", &self.path_env)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null());
        // Model settings and keys have nothing to do with a build, and an
        // inherited toolchain or build override (e.g. `RUSTUP_TOOLCHAIN`,
        // which `cargo` sets for everything it runs) would replace the
        // repository's own rust-toolchain.toml.
        for (k, _) in std::env::vars_os() {
            if let Some(k) = k.to_str() {
                if k.starts_with("CHIP_") || k.ends_with("_API_KEY") || is_build_override(k) {
                    c.env_remove(k);
                }
            }
        }
        crate::core::process_env::sanitize_std_command(&mut c);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000);
        }
        c
    }

    fn git(&self) -> Result<PathBuf, PrepareError> {
        self.find("git").ok_or_else(|| {
            PrepareError::new(
                "fetch",
                "git was not found on PATH; install git and try again",
            )
        })
    }

    fn cargo(&self) -> Result<PathBuf, PrepareError> {
        self.find("cargo")
            .or_else(|| {
                let home = dirs::home_dir()?;
                let p = home.join(".cargo").join("bin").join(if cfg!(windows) {
                    "cargo.exe"
                } else {
                    "cargo"
                });
                p.is_file().then_some(p)
            })
            .ok_or_else(|| {
                PrepareError::new(
                    "toolchain",
                    "cargo was not found: install Rust with rustup (https://rustup.rs); \
                     stack-agent's pinned toolchain is then installed automatically",
                )
            })
    }
}

const BUILD_OVERRIDES: &[&str] = &[
    "RUSTUP_TOOLCHAIN",
    "CARGO",
    "RUSTC",
    "RUSTDOC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTFLAGS",
    "RUSTDOCFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_TARGET_DIR",
    "CARGO_MANIFEST_DIR",
];
const BUILD_OVERRIDE_PREFIXES: &[&str] = &["CARGO_BUILD_", "CARGO_PROFILE_", "CARGO_TARGET_"];

/// Environment that would change what a build produces. Proxy, registry
/// and `CARGO_HOME` settings are kept.
pub fn is_build_override(name: &str) -> bool {
    BUILD_OVERRIDES.contains(&name) || BUILD_OVERRIDE_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Runs a command, streaming its output lines to `log`, within `timeout`.
fn stream(
    mut cmd: Command,
    timeout: Duration,
    log: &mut dyn FnMut(&str),
) -> Result<(bool, String), String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not start: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(l);
            }
        }));
    }
    if let Some(err) = child.stderr.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for l in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = tx.send(l);
            }
        }));
    }
    drop(tx);
    let start = Instant::now();
    let mut tail: Vec<String> = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                log(&line);
                tail.push(line);
                if tail.len() > 20 {
                    tail.remove(0);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "did not finish within {} minutes",
                timeout.as_secs() / 60
            ));
        }
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    Ok((status.success(), tail.join("\n")))
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

impl Source for GitSource {
    fn repository(&self) -> &str {
        &self.repository
    }

    fn latest(&self) -> Result<String, String> {
        let git = self.git().map_err(|e| e.message)?;
        let tmp = std::env::temp_dir();
        let mut cmd = self.command(&git, &tmp);
        cmd.args(["ls-remote", &self.repository, TRACKED_REF]);
        let out = super::validate::run_command(cmd, LS_REMOTE_TIMEOUT)?;
        if out.code != Some(0) {
            return Err(format!(
                "git ls-remote failed: {}",
                last_lines(&out.stderr, 2)
            ));
        }
        let commit = out
            .stdout
            .lines()
            .find_map(|l| {
                let mut parts = l.split_whitespace();
                let sha = parts.next()?;
                (parts.next()? == TRACKED_REF).then(|| sha.to_string())
            })
            .ok_or_else(|| format!("{} has no {TRACKED_REF}", self.repository))?;
        Ok(commit)
    }

    fn history(&self, commit: &str, n: usize, scratch: &Path) -> Result<Vec<String>, String> {
        if !is_full_commit(commit) {
            return Err(format!("{commit} is not a commit id"));
        }
        let git = self.git().map_err(|e| e.message)?;
        std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
        let run = |args: &[&str]| -> Result<String, String> {
            let mut cmd = self.command(&git, scratch);
            cmd.args(args);
            let out = super::validate::run_command(cmd, FETCH_TIMEOUT)?;
            if out.code == Some(0) {
                Ok(out.stdout)
            } else {
                Err(format!(
                    "git {} failed: {}",
                    args[0],
                    last_lines(&out.stderr, 2)
                ))
            }
        };
        run(&["init", "--quiet", "--template="])?;
        let depth = (n + 1).to_string();
        run(&[
            "fetch",
            "--quiet",
            "--depth",
            &depth,
            "--no-tags",
            &self.repository,
            commit,
        ])?;
        let list = run(&["rev-list", "--first-parent", "--max-count", &depth, commit])?;
        Ok(list
            .lines()
            .map(str::trim)
            .filter(|c| is_full_commit(c) && *c != commit)
            .map(str::to_string)
            .collect())
    }

    fn prepare(
        &self,
        commit: &str,
        staging: &Path,
        log: &mut dyn FnMut(&str),
    ) -> Result<Prepared, PrepareError> {
        if !is_full_commit(commit) {
            return Err(PrepareError::new(
                "fetch",
                format!("{commit} is not a commit id"),
            ));
        }
        let git = self.git()?;
        let src = staging.join("src");
        std::fs::create_dir_all(&src).map_err(|e| PrepareError::new("fetch", e.to_string()))?;
        let fetch_step =
            |label: &str, args: &[&str], log: &mut dyn FnMut(&str)| -> Result<(), PrepareError> {
                let mut cmd = self.command(&git, &src);
                cmd.args(args);
                match stream(cmd, FETCH_TIMEOUT, log) {
                    Ok((true, _)) => Ok(()),
                    Ok((false, tail)) => Err(PrepareError::new(
                        "fetch",
                        format!("git {label} failed: {}", last_lines(&tail, 3)),
                    )),
                    Err(e) => Err(PrepareError::new("fetch", format!("git {label}: {e}"))),
                }
            };
        log(&format!("Fetching {commit} from {}", self.repository));
        fetch_step("init", &["init", "--quiet", "--template="], log)?;
        // Only the exact commit is fetched and built, never a branch tip.
        fetch_step(
            "fetch",
            &[
                "-c",
                "core.hooksPath=",
                "fetch",
                "--quiet",
                "--depth",
                "1",
                "--no-tags",
                &self.repository,
                commit,
            ],
            log,
        )?;
        fetch_step(
            "checkout",
            &[
                "-c",
                "core.hooksPath=",
                "checkout",
                "--quiet",
                "--detach",
                "FETCH_HEAD",
            ],
            log,
        )?;
        let mut head = self.command(&git, &src);
        head.args(["rev-parse", "HEAD"]);
        let head = super::validate::run_command(head, LS_REMOTE_TIMEOUT)
            .map_err(|e| PrepareError::new("fetch", e))?;
        if head.stdout.trim() != commit {
            return Err(PrepareError::new(
                "fetch",
                format!("checked out {} instead of {commit}", head.stdout.trim()),
            ));
        }

        let cargo = self.cargo()?;
        // rustup's cargo honours the repository's rust-toolchain.toml; this
        // also installs that toolchain on first use.
        let exe = |name: &str| {
            cargo.with_file_name(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.to_string()
            })
        };
        let rustup = exe("rustup");
        if rustup.is_file() {
            // Recent rustup no longer installs a toolchain file's toolchain
            // implicitly; with no arguments this installs exactly that one.
            log("Installing the Rust toolchain pinned by rust-toolchain.toml (if missing)");
            let mut cmd = self.command(&rustup, &src);
            cmd.args(["toolchain", "install"]);
            if let Ok((false, tail)) = stream(cmd, FETCH_TIMEOUT, log) {
                log(&format!(
                    "rustup toolchain install did not succeed: {}",
                    last_lines(&tail, 2)
                ));
            }
        }
        let rustc = exe("rustc");
        let toolchain = {
            let mut cmd = self.command(
                if rustc.is_file() {
                    &rustc
                } else {
                    Path::new("rustc")
                },
                &src,
            );
            cmd.arg("-V");
            match stream(cmd, FETCH_TIMEOUT, log) {
                Ok((true, tail)) => tail
                    .lines()
                    .rev()
                    .find(|l| l.starts_with("rustc "))
                    .map(str::to_string),
                Ok((false, tail)) => {
                    return Err(PrepareError::new(
                        "toolchain",
                        format!(
                            "the pinned Rust toolchain is unavailable: {}",
                            last_lines(&tail, 3)
                        ),
                    ))
                }
                Err(e) => return Err(PrepareError::new("toolchain", e)),
            }
        };

        log("Building stack-agent (cargo build --release --locked --bin stack-agent)");
        std::fs::create_dir_all(&self.build_dir)
            .map_err(|e| PrepareError::new("build", e.to_string()))?;
        let mut cmd = self.command(&cargo, &src);
        // The same command stack-agent's release workflow builds with.
        cmd.args(["build", "--release", "--locked", "--bin", "stack-agent"])
            .env("CARGO_TARGET_DIR", &self.build_dir)
            .env("CARGO_TERM_COLOR", "never")
            .env("CARGO_TERM_PROGRESS_WHEN", "never");
        match stream(cmd, BUILD_TIMEOUT, log) {
            Ok((true, _)) => {}
            Ok((false, tail)) => {
                return Err(PrepareError::new(
                    "build",
                    format!("cargo build failed: {}", last_lines(&tail, 4)),
                ))
            }
            Err(e) => return Err(PrepareError::new("build", format!("cargo build {e}"))),
        }
        let built = self.build_dir.join("release").join(BIN_NAME);
        if !built.is_file() {
            return Err(PrepareError::new(
                "build",
                format!("cargo reported success but {} is missing", built.display()),
            ));
        }
        let bin = staging.join("bin").join(BIN_NAME);
        std::fs::create_dir_all(bin.parent().expect("has parent"))
            .map_err(|e| PrepareError::new("publish", e.to_string()))?;
        std::fs::copy(&built, &bin).map_err(|e| PrepareError::new("publish", e.to_string()))?;
        let _ = std::fs::remove_dir_all(&src);
        Ok(Prepared {
            binary: bin,
            toolchain,
        })
    }
}

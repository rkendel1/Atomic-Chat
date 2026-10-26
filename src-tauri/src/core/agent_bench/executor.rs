//! Drives one run through its lifecycle: prepare the working directory, run
//! the agent, run the task's verification commands, decide the outcome, and
//! save the record after every step so a crash leaves an honest trail.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::adapters::{self, Launch};
use super::changes;
use super::process::{self, Ending, Spec};
use super::redact::Redactor;
use super::run::*;
use super::store::Store;
use super::suite;
use super::task::{BenchTask, Evaluation, WorkdirRef, DEFAULT_CHECK_TIMEOUT_SECS};

/// Called with the record each time it is saved.
pub type Notify = std::sync::Arc<dyn Fn(&RunRecord) + Send + Sync>;

/// Builds the queued record for a run of `task` with the given agent.
pub fn new_record(store: &Store, task: &BenchTask, agent_id: &str, app_version: &str) -> RunRecord {
    let id = super::store::new_run_id();
    let workdir = match &task.workdir {
        WorkdirRef::Fixture { fixture } => WorkdirInfo {
            source: "fixture".to_string(),
            path: store
                .workspaces_dir()
                .join(&id)
                .to_string_lossy()
                .into_owned(),
            fixture: Some(fixture.clone()),
        },
        WorkdirRef::Path { path } => WorkdirInfo {
            source: "path".to_string(),
            path: path.clone(),
            fixture: None,
        },
    };
    let adapter = adapters::adapter(agent_id);
    let created_at = now();
    RunRecord {
        schema: RUN_SCHEMA.to_string(),
        id,
        task: TaskRef {
            id: task.id.clone(),
            version: task.version,
            digest: task.digest(),
            name: task.name.clone(),
            snapshot: task.clone(),
        },
        agent: AgentRef {
            id: agent_id.to_string(),
            name: adapter.map(|a| a.name).unwrap_or(agent_id).to_string(),
            execution_method: adapter
                .map(|a| a.execution_method)
                .unwrap_or("unsupported")
                .to_string(),
            executable: None,
            version: None,
            runtime: None,
        },
        model: None,
        provider: None,
        endpoint: None,
        config: Default::default(),
        credential_supplied: false,
        app_version: app_version.to_string(),
        workdir,
        status: RunStatus::Queued,
        failure_kind: None,
        created_at: created_at.clone(),
        started_at: None,
        finished_at: None,
        duration_ms: None,
        execution: None,
        verification: Vec::new(),
        changes: None,
        evaluation: EvaluationRecord {
            method: match task.evaluation {
                Evaluation::Command { .. } => "command",
                Evaluation::Manual { .. } => "manual",
            }
            .to_string(),
            verified: None,
            human: None,
            awaiting_review: false,
        },
        artifacts: Vec::new(),
        errors: Vec::new(),
        cancellation: None,
        interruption: None,
        history: vec![StatusChange {
            status: RunStatus::Queued,
            at: created_at,
        }],
    }
}

pub struct Context {
    pub store: Store,
    pub notify: Notify,
    pub cancel: CancellationToken,
    /// Runs one at a time, so durations are comparable and runs on the same
    /// project never overlap.
    pub slot: std::sync::Arc<tokio::sync::Semaphore>,
    /// Resolved by the caller; an error is recorded as an execution failure.
    pub launch: Result<Launch, String>,
    pub path_env: String,
}

impl Context {
    fn save(&self, run: &RunRecord) {
        if let Err(e) = self.store.save_run(run) {
            log::error!("agent bench: could not save run {}: {e}", run.id);
        }
        (self.notify)(run);
    }
}

pub async fn execute(ctx: Context, mut run: RunRecord) -> RunRecord {
    ctx.save(&run);
    let _permit = tokio::select! {
        permit = ctx.slot.clone().acquire_owned() => permit.ok(),
        _ = ctx.cancel.cancelled() => None,
    };
    if ctx.cancel.is_cancelled() {
        cancel(&mut run, "user");
        ctx.save(&run);
        return run;
    }
    let _ = run.transition(RunStatus::Running);
    ctx.save(&run);

    let task = run.task.snapshot.clone();
    let workdir = PathBuf::from(&run.workdir.path);
    if let Err(e) = prepare_workdir(&task, &workdir) {
        return fail_execution(&ctx, run, e, None);
    }
    let launch = match ctx.launch.clone() {
        Ok(l) => l,
        Err(e) => return fail_execution(&ctx, run, e, None),
    };
    let redactor = Redactor::new(launch.secrets.clone());
    run.agent.executable = Some(launch.program.to_string_lossy().into_owned());
    run.model = launch.model.clone();
    run.provider = launch.provider.clone();
    run.endpoint = launch.endpoint.as_deref().map(|e| redactor.text(e));
    run.config = launch
        .config
        .iter()
        .map(|(k, v)| (k.clone(), redactor.text(v)))
        .collect();
    run.credential_supplied = launch.secrets.iter().any(|s| !s.is_empty());
    run.agent.version = agent_version(&launch, &workdir, &ctx.path_env).await;
    ctx.save(&run);

    if let Some(url) = &launch.preflight_url {
        if let Err(e) = check_reachable(url).await {
            return fail_execution(&ctx, run, e, Some(&launch));
        }
    }

    let before = changes::snapshot(&workdir);
    let mut env = launch.env.clone();
    env.push(("PATH".to_string(), ctx.path_env.clone()));
    let spec = Spec {
        program: &launch.program,
        args: &launch.args,
        cwd: &workdir,
        env: &env,
        env_remove: &launch.env_remove,
        timeout: launch.timeout,
        stdout_limit: process::STDOUT_LIMIT,
        stderr_limit: process::STDERR_LIMIT,
    };
    let command_line = redactor.text(&display_command(&launch.program, &launch.args));
    let finished = match process::run(spec, &ctx.cancel).await {
        Ok(f) => f,
        Err(e) => return fail_execution(&ctx, run, e, Some(&launch)),
    };
    let stdout = process::captured(
        &finished.stdout,
        finished.stdout_total,
        process::STDOUT_LIMIT,
    );
    let stderr = process::captured(
        &finished.stderr,
        finished.stderr_total,
        process::STDERR_LIMIT,
    );
    let report = adapters::parse_report(&String::from_utf8_lossy(&finished.stdout));
    if let Some(r) = &report {
        run.artifacts
            .extend(adapters::report_artifacts(&run.agent.id, r));
    }
    let outcome = match finished.ending {
        Ending::TimedOut => ExecutionOutcome::TimedOut,
        Ending::Cancelled => ExecutionOutcome::Cancelled,
        Ending::Exited if finished.exit_code == Some(0) => ExecutionOutcome::Exited,
        Ending::Exited => ExecutionOutcome::ExitedNonZero,
    };
    let error = match outcome {
        ExecutionOutcome::TimedOut => Some(format!(
            "stopped after the time limit ({} s)",
            launch.timeout.as_secs()
        )),
        ExecutionOutcome::ExitedNonZero | ExecutionOutcome::Cancelled => {
            last_error_lines(&stderr.text)
        }
        _ => None,
    }
    .map(|e| redactor.text(&e));
    run.execution = Some(ExecutionRecord {
        outcome,
        command_line,
        exit_code: finished.exit_code,
        signal: finished.signal,
        duration_ms: finished.duration.as_millis() as u64,
        stdout: CapturedText {
            text: redactor.text(&stdout.text),
            ..stdout
        },
        stderr: CapturedText {
            text: redactor.text(&stderr.text),
            ..stderr
        },
        agent_report: report.map(|r| redactor.json(&r)),
        error,
    });
    run.changes = Some(changes::compare(&before, &changes::snapshot(&workdir)));
    run.artifacts.push(Artifact {
        kind: "workdir".to_string(),
        reference: run.workdir.path.clone(),
        description: if run.workdir.source == "fixture" {
            "Isolated copy of the fixture, as the agent left it".to_string()
        } else {
            "The project the agent changed in place".to_string()
        },
    });
    if outcome == ExecutionOutcome::TimedOut {
        run.cancellation = Some(Cancellation {
            requested_at: now(),
            reason: "timeout".to_string(),
        });
    }

    if outcome == ExecutionOutcome::Cancelled {
        cancel(&mut run, "user");
        ctx.save(&run);
        return run;
    }

    let _ = run.transition(RunStatus::Evaluating);
    ctx.save(&run);
    evaluate(&ctx, &mut run, &task, &workdir, &redactor).await;
    ctx.save(&run);
    run
}

async fn evaluate(
    ctx: &Context,
    run: &mut RunRecord,
    task: &BenchTask,
    workdir: &Path,
    redactor: &Redactor,
) {
    match &task.evaluation {
        Evaluation::Manual { .. } => {
            run.evaluation.awaiting_review = true;
        }
        Evaluation::Command {
            commands,
            held_out_checks,
        } => {
            if let Some(set) = held_out_checks {
                let files = suite::held_out_files(set).unwrap_or(&[]);
                if let Err(e) = suite::write_files(workdir, files) {
                    run.errors
                        .push(format!("could not install held-out checks: {e}"));
                    run.evaluation.verified = Some(Verdict::Inconclusive);
                    run.failure_kind = Some(FailureKind::VerificationUnavailable);
                    let _ = run.transition(RunStatus::Inconclusive);
                    return;
                }
            }
            for c in commands {
                let result = if ctx.cancel.is_cancelled() {
                    CheckResult {
                        name: c.name.clone(),
                        command_line: c.display(),
                        outcome: CheckOutcome::Skipped,
                        exit_code: None,
                        duration_ms: 0,
                        stdout: Default::default(),
                        stderr: Default::default(),
                        error: Some("run cancelled".to_string()),
                    }
                } else {
                    run_check(c, workdir, &ctx.path_env, &ctx.cancel, redactor).await
                };
                run.verification.push(result);
                ctx.save(run);
            }
            if ctx.cancel.is_cancelled() {
                cancel(run, "user");
                return;
            }
            let verdict = verdict_of(&run.verification);
            run.evaluation.verified = Some(verdict);
            let (status, failure) = match verdict {
                Verdict::Success => (RunStatus::Succeeded, None),
                Verdict::Failure => (RunStatus::Failed, Some(FailureKind::Verification)),
                Verdict::Inconclusive => (
                    RunStatus::Inconclusive,
                    Some(FailureKind::VerificationUnavailable),
                ),
            };
            run.failure_kind = failure;
            let _ = run.transition(status);
        }
    }
}

/// Success only when every check passed; a check that failed or timed out
/// is a failure; otherwise (a check could not run) nothing was established.
pub fn verdict_of(checks: &[CheckResult]) -> Verdict {
    if checks.is_empty() {
        return Verdict::Inconclusive;
    }
    if checks
        .iter()
        .any(|c| matches!(c.outcome, CheckOutcome::Failed | CheckOutcome::TimedOut))
    {
        return Verdict::Failure;
    }
    if checks.iter().all(|c| c.outcome == CheckOutcome::Passed) {
        Verdict::Success
    } else {
        Verdict::Inconclusive
    }
}

async fn run_check(
    c: &super::task::VerificationCommand,
    workdir: &Path,
    path_env: &str,
    cancel: &CancellationToken,
    redactor: &Redactor,
) -> CheckResult {
    let command_line = c.display();
    let program = resolve_program(&c.program, path_env);
    let env = vec![("PATH".to_string(), path_env.to_string())];
    let spec = Spec {
        program: &program,
        args: &c.args,
        cwd: workdir,
        env: &env,
        env_remove: &[],
        timeout: Duration::from_secs(c.timeout_secs.unwrap_or(DEFAULT_CHECK_TIMEOUT_SECS)),
        stdout_limit: process::STDERR_LIMIT,
        stderr_limit: process::STDERR_LIMIT,
    };
    match process::run(spec, cancel).await {
        Err(e) => CheckResult {
            name: c.name.clone(),
            command_line,
            outcome: CheckOutcome::FailedToStart,
            exit_code: None,
            duration_ms: 0,
            stdout: Default::default(),
            stderr: Default::default(),
            error: Some(redactor.text(&e)),
        },
        Ok(f) => {
            let outcome = match f.ending {
                Ending::TimedOut => CheckOutcome::TimedOut,
                Ending::Cancelled => CheckOutcome::Skipped,
                Ending::Exited if f.exit_code == Some(0) => CheckOutcome::Passed,
                Ending::Exited => CheckOutcome::Failed,
            };
            let out = process::captured(&f.stdout, f.stdout_total, process::STDERR_LIMIT);
            let err = process::captured(&f.stderr, f.stderr_total, process::STDERR_LIMIT);
            CheckResult {
                name: c.name.clone(),
                command_line,
                outcome,
                exit_code: f.exit_code,
                duration_ms: f.duration.as_millis() as u64,
                stdout: CapturedText {
                    text: redactor.text(&out.text),
                    ..out
                },
                stderr: CapturedText {
                    text: redactor.text(&err.text),
                    ..err
                },
                error: None,
            }
        }
    }
}

/// A bare program name is looked up on the child's PATH (a GUI process's own
/// PATH may not have it); anything with a separator is used as given.
fn resolve_program(program: &str, path_env: &str) -> PathBuf {
    if program.contains('/') || program.contains('\\') {
        return PathBuf::from(program);
    }
    adapters::search_path(program, path_env).unwrap_or_else(|| PathBuf::from(program))
}

fn prepare_workdir(task: &BenchTask, workdir: &Path) -> Result<(), String> {
    match &task.workdir {
        WorkdirRef::Fixture { fixture } => {
            if workdir.exists() {
                return Err(format!(
                    "workspace {} already exists; refusing to reuse it",
                    workdir.display()
                ));
            }
            suite::materialize(fixture, workdir)
        }
        WorkdirRef::Path { .. } => {
            if workdir.is_dir() {
                Ok(())
            } else {
                Err(format!(
                    "project directory {} does not exist",
                    workdir.display()
                ))
            }
        }
    }
}

fn fail_execution(
    ctx: &Context,
    mut run: RunRecord,
    error: String,
    launch: Option<&Launch>,
) -> RunRecord {
    let redactor = Redactor::new(launch.map(|l| l.secrets.clone()).unwrap_or_default());
    let error = redactor.text(&error);
    run.execution = Some(ExecutionRecord {
        outcome: ExecutionOutcome::FailedToStart,
        command_line: launch
            .map(|l| redactor.text(&display_command(&l.program, &l.args)))
            .unwrap_or_default(),
        exit_code: None,
        signal: None,
        duration_ms: 0,
        stdout: Default::default(),
        stderr: Default::default(),
        agent_report: None,
        error: Some(error.clone()),
    });
    run.errors.push(error);
    run.failure_kind = Some(FailureKind::Execution);
    let _ = run.transition(RunStatus::Failed);
    ctx.save(&run);
    run
}

fn cancel(run: &mut RunRecord, reason: &str) {
    if run.cancellation.is_none() {
        run.cancellation = Some(Cancellation {
            requested_at: now(),
            reason: reason.to_string(),
        });
    }
    run.failure_kind = Some(FailureKind::Cancelled);
    let _ = run.transition(RunStatus::Cancelled);
}

async fn agent_version(launch: &Launch, cwd: &Path, path_env: &str) -> Option<String> {
    let args = vec!["--version".to_string()];
    let env = vec![("PATH".to_string(), path_env.to_string())];
    let spec = Spec {
        program: &launch.program,
        args: &args,
        cwd,
        env: &env,
        env_remove: &[],
        timeout: Duration::from_secs(20),
        stdout_limit: 4096,
        stderr_limit: 4096,
    };
    let f = process::run(spec, &CancellationToken::new()).await.ok()?;
    if f.ending != Ending::Exited || f.exit_code != Some(0) {
        return None;
    }
    String::from_utf8_lossy(&f.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(200).collect())
}

/// Fails fast, with what to do, when nothing listens at the model endpoint.
pub async fn check_reachable(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid endpoint {url}: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("endpoint {url} has no host"))?
        .trim_matches(|c| c == '[' || c == ']')
        .to_string();
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| format!("endpoint {url} has no port"))?;
    let connect = tokio::net::TcpStream::connect((host.as_str(), port));
    match tokio::time::timeout(Duration::from_secs(3), connect).await {
        Ok(Ok(_)) => Ok(()),
        _ => Err(format!(
            "The model endpoint {url} is not reachable. Start the Local API Server \
             (Server → Overview) with a model loaded, then run again."
        )),
    }
}

fn display_command(program: &Path, args: &[String]) -> String {
    let mut parts = vec![program.to_string_lossy().into_owned()];
    for a in args {
        if a.is_empty() || a.contains(char::is_whitespace) || a.contains('"') {
            parts.push(format!("\"{}\"", a.replace('"', "\\\"")));
        } else {
            parts.push(a.clone());
        }
    }
    parts.join(" ")
}

/// The last `error:` lines an agent wrote, or its last non-empty line.
fn last_error_lines(stderr: &str) -> Option<String> {
    let errors: Vec<&str> = stderr
        .lines()
        .filter(|l| l.trim_start().to_ascii_lowercase().starts_with("error"))
        .collect();
    let picked = if errors.is_empty() {
        stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())?
            .to_string()
    } else {
        errors[errors.len().saturating_sub(3)..].join("\n")
    };
    Some(picked.chars().take(2000).collect())
}

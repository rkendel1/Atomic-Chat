//! Drives one run through its lifecycle: prepare the working directory, run
//! the agent, run the task's verification commands, decide the outcome, and
//! save the record after every step so a crash leaves an honest trail.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::adapters::{self, Launch};
use super::baseline;
use super::changes;
use super::evidence::{self, AgentTrace};
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
        WorkdirRef::GitSnapshot { commit, .. } => WorkdirInfo {
            source: "git_snapshot".to_string(),
            path: store
                .workspaces_dir()
                .join(&id)
                .to_string_lossy()
                .into_owned(),
            fixture: Some(format!("git:{commit}")),
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
        baseline: None,
        agent_trace: None,
        analysis: None,
        suite: None,
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
    let prepared = {
        let (w, d, p) = (task.workdir.clone(), workdir.clone(), ctx.path_env.clone());
        tokio::task::spawn_blocking(move || baseline::prepare(&w, &d, &p))
            .await
            .unwrap_or_else(|e| Err(e.to_string()))
    };
    match prepared {
        Ok(b) => run.baseline = Some(b),
        Err(e) => return fail_execution(&ctx, run, e, None),
    }
    let launch = match ctx.launch.clone() {
        Ok(l) => l,
        Err(e) => return fail_execution(&ctx, run, e, None),
    };
    // The recorded runtime must be the file about to run: a managed
    // stack-agent is checked against the digest it was resolved with.
    if let Some(expected) = run
        .agent
        .runtime
        .as_ref()
        .and_then(|r| r.binary_sha256.clone())
    {
        let program = launch.program.clone();
        let actual = tokio::task::spawn_blocking(move || {
            crate::core::stack_agent_runtime::sha256_file(&program)
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        match actual {
            Ok(d) if d == expected => {}
            Ok(d) => {
                let e = format!(
                    "the stack-agent executable changed after it was resolved (sha256 {d}, recorded {expected}); not running it"
                );
                return fail_execution(&ctx, run, e, None);
            }
            Err(e) => return fail_execution(&ctx, run, e, None),
        }
    }
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
    let agent_started_at = now();
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
        started_at: Some(agent_started_at),
        ended_at: Some(now()),
    });
    run.changes = Some(changes::compare(&before, &changes::snapshot(&workdir)));
    let report_ref = run.execution.as_ref().and_then(|e| e.agent_report.clone());
    run.agent_trace = Some(
        collect_trace(
            &run.agent.id,
            &launch,
            report_ref.as_ref(),
            &stderr.text,
            &workdir,
            &ctx.path_env,
            &redactor,
        )
        .await,
    );
    run.artifacts.push(Artifact {
        kind: "workdir".to_string(),
        reference: run.workdir.path.clone(),
        description: if run.workdir.source != "path" {
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
            run.reanalyze();
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
                        started_at: None,
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
    let started_at = Some(now());
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
            started_at,
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
                started_at,
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
        started_at: None,
        ended_at: None,
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

/// The last `error:` lines an agent wrote, or its last non-empty line. When
/// the agent printed model replies, only `error:` lines count: the last line
/// would be part of a reply, i.e. the model's words.
fn last_error_lines(stderr: &str) -> Option<String> {
    let errors: Vec<&str> = stderr
        .lines()
        .filter(|l| l.trim_start().to_ascii_lowercase().starts_with("error"))
        .collect();
    let picked = if errors.is_empty() {
        if stderr.contains("Model reply:\n") {
            return None;
        }
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

/// Reads the agent's own trajectory through its adapter's interface, and the
/// model replies it printed. Whatever cannot be read is recorded as missing,
/// with the reason.
async fn collect_trace(
    agent_id: &str,
    launch: &Launch,
    report: Option<&serde_json::Value>,
    stderr: &str,
    cwd: &Path,
    path_env: &str,
    redactor: &Redactor,
) -> AgentTrace {
    let mut trace = AgentTrace {
        source: adapters::STACK_AGENT_TRACE_SOURCE.to_string(),
        ..Default::default()
    };
    if agent_id != "stack-agent" {
        trace.source = format!("{agent_id} (no trajectory interface)");
    }
    if launch.config.get("print_reply").map(String::as_str) == Some("true") {
        trace.replies = evidence::parse_print_replies(stderr, 8 * 1024)
            .into_iter()
            .map(|mut r| {
                r.text = redactor.text(&r.text);
                r
            })
            .collect();
    } else {
        trace.replies_unavailable_reason = Some(format!(
            "{agent_id} does not print its model replies to Agent Bench"
        ));
    }
    let args = match adapters::trace_command(agent_id, launch, report) {
        Ok(a) => a,
        Err(e) => {
            trace.unavailable_reason = Some(redactor.text(&e));
            return trace;
        }
    };
    let env = vec![("PATH".to_string(), path_env.to_string())];
    let spec = Spec {
        program: &launch.program,
        args: &args,
        cwd,
        env: &env,
        env_remove: &[],
        timeout: Duration::from_secs(60),
        stdout_limit: process::STDOUT_LIMIT,
        stderr_limit: 16 * 1024,
    };
    match process::run(spec, &CancellationToken::new()).await {
        Ok(f) if f.ending == Ending::Exited && f.exit_code == Some(0) => {
            match serde_json::from_slice::<serde_json::Value>(&f.stdout) {
                Ok(show) => {
                    trace.events = adapters::stack_agent_trace(&show, redactor);
                    trace.available = true;
                }
                Err(e) => {
                    trace.unavailable_reason =
                        Some(format!("agent-state show printed no readable JSON: {e}"))
                }
            }
        }
        Ok(f) => {
            trace.unavailable_reason = Some(redactor.text(&format!(
                "agent-state show did not succeed (exit {:?}): {}",
                f.exit_code,
                String::from_utf8_lossy(&f.stderr).trim()
            )))
        }
        Err(e) => trace.unavailable_reason = Some(redactor.text(&e)),
    }
    trace
}

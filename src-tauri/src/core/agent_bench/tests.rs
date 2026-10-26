//! Agent Bench tests. Agents are fake executables (shell scripts) and the
//! model endpoint is a local TCP listener: no model, network or installed
//! agent is needed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::adapters::{self, RunConfig};
use super::executor::{self, Context};
use super::export;
use super::redact::{Redactor, REDACTED};
use super::run::*;
use super::store::Store;
use super::suite;
use super::task::*;

const KEY: &str = "sk-test-0123456789abcdefghij";

fn path_task(dir: &Path, commands: Vec<VerificationCommand>) -> BenchTask {
    BenchTask {
        schema: TASK_SCHEMA.to_string(),
        id: "demo-task".to_string(),
        version: 1,
        name: "Demo".to_string(),
        description: String::new(),
        goal: "Create done.txt".to_string(),
        workdir: WorkdirRef::Path {
            path: dir.to_string_lossy().into_owned(),
        },
        success_criteria: vec!["done.txt exists".to_string()],
        evaluation: Evaluation::Command {
            commands,
            held_out_checks: None,
        },
        limits: Limits {
            timeout_secs: Some(30),
            max_turns: Some(4),
        },
        builtin: false,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn check(name: &str, program: &str, args: &[&str]) -> VerificationCommand {
    VerificationCommand {
        name: name.to_string(),
        program: program.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        timeout_secs: Some(30),
    }
}

// ---- task contract --------------------------------------------------------

#[test]
fn task_round_trips_through_json_with_its_schema() {
    let dir = tempfile::tempdir().unwrap();
    let task = path_task(dir.path(), vec![check("ok", "true", &[])]);
    let text = serde_json::to_string(&task).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["schema"], TASK_SCHEMA);
    assert_eq!(value["workdir"]["kind"], "path");
    assert_eq!(value["evaluation"]["method"], "command");
    assert_eq!(serde_json::from_str::<BenchTask>(&text).unwrap(), task);
    task.validate().unwrap();
}

#[test]
fn manual_task_deserializes_without_optional_fields() {
    let task: BenchTask = serde_json::from_value(serde_json::json!({
        "schema": TASK_SCHEMA, "id": "m", "version": 1, "name": "M", "goal": "g",
        "workdir": {"kind": "fixture", "fixture": "ledger"},
        "evaluation": {"method": "manual"}
    }))
    .unwrap();
    assert!(matches!(task.evaluation, Evaluation::Manual { .. }));
    assert_eq!(task.limits, Limits::default());
    task.validate().unwrap();
}

#[test]
fn validation_rejects_bad_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let good = path_task(dir.path(), vec![check("ok", "true", &[])]);
    let mut t = good.clone();
    t.schema = "atomic.bench.task.v0".into();
    assert!(t.validate().unwrap_err().contains("schema"));
    let mut t = good.clone();
    t.id = "../escape".into();
    assert!(t.validate().is_err());
    let mut t = good.clone();
    t.evaluation = Evaluation::Command {
        commands: vec![],
        held_out_checks: None,
    };
    assert!(t.validate().is_err());
    let mut t = good.clone();
    t.workdir = WorkdirRef::Path {
        path: "relative/dir".into(),
    };
    assert!(t.validate().is_err());
    let mut t = good.clone();
    t.evaluation = Evaluation::Command {
        commands: vec![check("ok", "true", &[])],
        held_out_checks: Some("ledger-answer".into()),
    };
    assert!(t.validate().unwrap_err().contains("fixture"));
    let mut t = good;
    t.limits.timeout_secs = Some(0);
    assert!(t.validate().is_err());
}

#[test]
fn digest_tracks_content_not_bookkeeping() {
    let dir = tempfile::tempdir().unwrap();
    let a = path_task(dir.path(), vec![check("ok", "true", &[])]);
    let mut b = a.clone();
    b.version = 7;
    b.updated_at = "later".into();
    assert_eq!(a.digest(), b.digest());
    b.goal = "Something else".into();
    assert_ne!(a.digest(), b.digest());
}

#[test]
fn saving_bumps_the_version_only_when_content_changes() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = path_task(project.path(), vec![check("ok", "true", &[])]);
    let v1 = store.save_task(task.clone()).unwrap();
    assert_eq!(v1.version, 1);
    let same = store.save_task(task.clone()).unwrap();
    assert_eq!(same.version, 1);
    let mut changed = task;
    changed.goal = "Create finished.txt".into();
    let v2 = store.save_task(changed).unwrap();
    assert_eq!(v2.version, 2);
    // Persisted across a new store (an app restart).
    let reopened = Store::new(data.path());
    assert_eq!(reopened.get_task("demo-task").unwrap().version, 2);
    assert!(reopened
        .list_tasks()
        .unwrap()
        .iter()
        .any(|t| t.id == "demo-task" && !t.builtin));
}

#[test]
fn bundled_tasks_are_valid_and_read_only() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let builtins = suite::builtin_tasks();
    assert!(builtins.len() >= 3);
    for t in &builtins {
        t.validate().unwrap();
        assert!(t.builtin);
        assert!(matches!(t.evaluation, Evaluation::Command { .. }));
    }
    assert!(store.save_task(builtins[0].clone()).is_err());
    assert!(store.delete_task(&builtins[0].id).is_err());
}

#[test]
fn prompt_lists_the_success_criteria() {
    let dir = tempfile::tempdir().unwrap();
    let t = path_task(dir.path(), vec![check("ok", "true", &[])]);
    assert_eq!(
        t.prompt(),
        "Create done.txt\n\nSuccess criteria:\n- done.txt exists"
    );
}

#[test]
fn fixtures_materialize_every_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    suite::materialize("ledger-discount-bug", &ws).unwrap();
    assert!(ws.join("Cargo.toml").is_file());
    assert!(ws.join(".gitignore").is_file());
    let pricing = std::fs::read_to_string(ws.join("src/pricing.rs")).unwrap();
    assert!(
        pricing.contains("/ 100"),
        "the bug variant replaces pricing.rs"
    );
    assert!(suite::materialize("nope", &dir.path().join("x")).is_err());
}

// ---- lifecycle ------------------------------------------------------------

fn record(store: &Store, task: &BenchTask) -> RunRecord {
    executor::new_record(store, task, "stack-agent", "9.9.9")
}

#[test]
fn lifecycle_allows_only_forward_transitions() {
    use RunStatus::*;
    assert!(Queued.can_become(Running));
    assert!(Running.can_become(Evaluating));
    assert!(Evaluating.can_become(Succeeded));
    assert!(
        !Queued.can_become(Succeeded),
        "nothing succeeds unevaluated"
    );
    assert!(
        !Running.can_become(Succeeded),
        "nothing succeeds unevaluated"
    );
    assert!(!Evaluating.can_become(Running));
    for t in [Succeeded, Failed, Inconclusive, Cancelled] {
        for n in [
            Queued,
            Running,
            Evaluating,
            Succeeded,
            Failed,
            Inconclusive,
            Cancelled,
        ] {
            assert!(!t.can_become(n), "{t:?} is terminal");
        }
    }
}

#[test]
fn transitions_record_history_and_duration() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = path_task(data.path(), vec![check("ok", "true", &[])]);
    let mut run = record(&store, &task);
    run.transition(RunStatus::Running).unwrap();
    assert!(run.started_at.is_some());
    assert!(run.transition(RunStatus::Succeeded).is_err());
    run.transition(RunStatus::Evaluating).unwrap();
    run.transition(RunStatus::Failed).unwrap();
    assert!(run.finished_at.is_some() && run.duration_ms.is_some());
    let statuses: Vec<_> = run.history.iter().map(|h| h.status).collect();
    assert_eq!(
        statuses,
        vec![
            RunStatus::Queued,
            RunStatus::Running,
            RunStatus::Evaluating,
            RunStatus::Failed
        ]
    );
    assert!(run.transition(RunStatus::Succeeded).is_err());
}

// ---- persistence and recovery ----------------------------------------------

#[test]
fn runs_persist_and_stale_ones_recover_as_inconclusive() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = path_task(data.path(), vec![check("ok", "true", &[])]);

    let mut running = record(&store, &task);
    running.transition(RunStatus::Running).unwrap();
    store.save_run(&running).unwrap();

    let mut evaluating = record(&store, &task);
    evaluating.transition(RunStatus::Running).unwrap();
    evaluating.transition(RunStatus::Evaluating).unwrap();
    store.save_run(&evaluating).unwrap();

    let mut awaiting = record(&store, &task);
    awaiting.transition(RunStatus::Running).unwrap();
    awaiting.transition(RunStatus::Evaluating).unwrap();
    awaiting.evaluation.awaiting_review = true;
    store.save_run(&awaiting).unwrap();

    let mut done = record(&store, &task);
    done.transition(RunStatus::Running).unwrap();
    done.transition(RunStatus::Evaluating).unwrap();
    done.transition(RunStatus::Succeeded).unwrap();
    store.save_run(&done).unwrap();

    // A new process.
    let store = Store::new(data.path());
    let mut recovered = store.recover_stale_runs().unwrap();
    recovered.sort();
    let mut expected = vec![running.id.clone(), evaluating.id.clone()];
    expected.sort();
    assert_eq!(recovered, expected);

    let r = store.get_run(&running.id).unwrap();
    assert_eq!(r.status, RunStatus::Inconclusive);
    assert_eq!(r.failure_kind, Some(FailureKind::Interrupted));
    assert_eq!(
        r.interruption.as_ref().unwrap().previous_status,
        RunStatus::Running
    );
    assert_ne!(r.status, RunStatus::Succeeded);
    assert_eq!(
        store.get_run(&awaiting.id).unwrap().status,
        RunStatus::Evaluating,
        "a run waiting for review was not interrupted"
    );
    assert_eq!(
        store.get_run(&done.id).unwrap().status,
        RunStatus::Succeeded
    );
    assert_eq!(store.list_runs().unwrap().len(), 4);
    assert!(store.recover_stale_runs().unwrap().is_empty());
}

#[test]
fn a_damaged_record_does_not_hide_the_others() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = path_task(data.path(), vec![check("ok", "true", &[])]);
    store.save_run(&record(&store, &task)).unwrap();
    std::fs::write(
        data.path().join("agent_bench/runs/run_broken.json"),
        "{not json",
    )
    .unwrap();
    assert_eq!(store.list_runs().unwrap().len(), 1);
    assert!(store.get_run("../../etc/passwd").is_err());
}

// ---- manual evaluation ------------------------------------------------------

#[test]
fn manual_judgment_decides_only_a_run_awaiting_review() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let mut task = path_task(data.path(), vec![]);
    task.evaluation = Evaluation::Manual {
        instructions: "Read the diff".into(),
    };
    let mut run = record(&store, &task);
    assert!(run.apply_judgment(Verdict::Success, String::new()).is_err());
    run.transition(RunStatus::Running).unwrap();
    run.transition(RunStatus::Evaluating).unwrap();
    run.evaluation.awaiting_review = true;
    run.apply_judgment(Verdict::Failure, "Wrong file".into())
        .unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.failure_kind, Some(FailureKind::HumanJudgment));
    assert_eq!(run.evaluation.human.as_ref().unwrap().note, "Wrong file");
    assert!(run.evaluation.verified.is_none());

    // On a verified run a judgment is kept beside the outcome, never replacing it.
    let mut verified = record(&store, &task);
    verified.transition(RunStatus::Running).unwrap();
    verified.transition(RunStatus::Evaluating).unwrap();
    verified.evaluation.verified = Some(Verdict::Failure);
    verified.transition(RunStatus::Failed).unwrap();
    verified
        .apply_judgment(Verdict::Success, "Looks fine to me".into())
        .unwrap();
    assert_eq!(verified.status, RunStatus::Failed);
    assert_eq!(verified.evaluation.verified, Some(Verdict::Failure));
}

// ---- redaction ------------------------------------------------------------

#[test]
fn redaction_removes_known_and_shaped_credentials() {
    let r = Redactor::new([KEY.to_string(), "atomic".to_string()]);
    let text = format!(
        "key {KEY} CHIP_API_KEY={KEY} Authorization: Bearer abcdefghijklmnopqrstuvwxyz \
         OPENAI_API_KEY=sk-otherotherotherother123 atomic chat"
    );
    let out = r.text(&text);
    assert!(!out.contains(KEY), "{out}");
    assert!(!out.contains("sk-otherother"), "{out}");
    assert!(!out.contains("abcdefghijklmnop"), "{out}");
    assert!(out.contains(REDACTED));
    assert!(
        out.contains("atomic chat"),
        "short placeholders are left alone"
    );

    let json = serde_json::json!({
        "api_key": "whatever", "apiKey": "x", "nested": [{"msg": format!("used {KEY}")}],
        "usage": {"input_tokens": 12, "output_tokens": 3}, "max_tokens": 5,
        "credential_supplied": true
    });
    let out = r.json(&json);
    assert_eq!(out["api_key"], REDACTED);
    assert_eq!(out["apiKey"], REDACTED);
    assert!(!out.to_string().contains(KEY));
    assert_eq!(
        out["usage"]["input_tokens"], 12,
        "token counts are not secrets"
    );
    assert_eq!(out["max_tokens"], 5);
    assert_eq!(out["credential_supplied"], true);
}

// ---- adapters ---------------------------------------------------------------

fn cfg(agent: &str, api_url: Option<&str>) -> RunConfig {
    RunConfig {
        agent_id: agent.to_string(),
        model: Some("qwen3-8b".to_string()),
        api_url: api_url.map(str::to_string),
        api_key: Some(KEY.to_string()),
        executable: None,
    }
}

#[test]
fn stack_agent_adapter_uses_the_documented_work_interface() {
    let dir = tempfile::tempdir().unwrap();
    let mut task = path_task(dir.path(), vec![check("ok", "true", &[])]);
    task.limits.max_turns = Some(99);
    let launch = adapters::plan(
        &task,
        &cfg("stack-agent", Some("http://127.0.0.1:1337/v1")),
        Path::new("/bin/stack-agent"),
    )
    .unwrap();
    assert_eq!(launch.args[0], "work");
    assert_eq!(launch.args[1], task.prompt());
    let joined = launch.args.join(" ");
    for flag in [
        "--kind change",
        "--json",
        "--deadline-secs 30",
        "--max-turns 50",
    ] {
        assert!(joined.contains(flag), "{joined}");
    }
    let env: std::collections::HashMap<_, _> = launch.env.iter().cloned().collect();
    assert_eq!(
        env["CHIP_ENDPOINT"],
        "http://127.0.0.1:1337/v1/chat/completions"
    );
    assert_eq!(env["CHIP_MODEL"], "qwen3-8b");
    assert_eq!(env["CHIP_API_KEY"], KEY, "the key reaches the child");
    assert!(
        !launch.args.iter().any(|a| a.contains(KEY)),
        "never in argv"
    );
    assert!(
        !format!("{:?}", launch.config).contains(KEY),
        "never in config"
    );
    assert!(launch.config.contains_key("CHIP_MODEL"));
    assert!(
        launch.timeout > Duration::from_secs(30),
        "the agent reports its own deadline"
    );
    assert!(launch.env_remove.contains(&"OPENAI_API_KEY".to_string()));
    assert_eq!(
        launch.provider.as_deref(),
        Some(adapters::LOCAL_API_PROVIDER)
    );

    let missing = adapters::plan(
        &task,
        &RunConfig {
            model: None,
            ..cfg("stack-agent", Some("http://127.0.0.1:1337/v1"))
        },
        Path::new("/bin/stack-agent"),
    );
    assert!(missing.unwrap_err().contains("model"));
    assert!(adapters::plan(&task, &cfg("stack-agent", None), Path::new("/x")).is_err());
    assert!(adapters::plan(&task, &cfg("nobody", None), Path::new("/x")).is_err());
}

#[test]
fn claude_code_adapter_runs_print_mode() {
    let dir = tempfile::tempdir().unwrap();
    let task = path_task(dir.path(), vec![check("ok", "true", &[])]);
    let launch = adapters::plan(
        &task,
        &cfg("claude-code", Some("http://127.0.0.1:1337")),
        Path::new("/bin/claude"),
    )
    .unwrap();
    assert_eq!(launch.args[0], "-p");
    let joined = launch.args.join(" ");
    assert!(joined.contains("--output-format json"));
    assert!(joined.contains("--permission-mode acceptEdits"));
    assert!(joined.contains("--model qwen3-8b"));
    assert!(launch
        .env
        .contains(&("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:1337".into())));
    assert!(launch.env_remove.contains(&"ANTHROPIC_API_KEY".to_string()));
    assert_eq!(
        launch.provider.as_deref(),
        Some(adapters::LOCAL_API_PROVIDER)
    );
    assert!(!format!("{:?}", launch.config).contains(KEY));

    let own = adapters::plan(&task, &cfg("claude-code", None), Path::new("/bin/claude")).unwrap();
    assert!(own.env.is_empty());
    assert!(own.preflight_url.is_none());
}

#[test]
fn reports_parse_from_noisy_stdout() {
    assert!(adapters::parse_report("{\"terminal_state\":\"completed\"}").is_some());
    assert!(adapters::parse_report("progress\n{\"a\":1}\n").is_some());
    assert!(adapters::parse_report("not json").is_none());
    assert!(adapters::parse_report("[1,2]").is_none());
}

#[test]
fn verdict_needs_every_check_to_pass() {
    let mk = |outcome| CheckResult {
        name: "c".into(),
        command_line: "c".into(),
        outcome,
        exit_code: None,
        duration_ms: 0,
        stdout: Default::default(),
        stderr: Default::default(),
        error: None,
    };
    use CheckOutcome::*;
    assert_eq!(executor::verdict_of(&[]), Verdict::Inconclusive);
    assert_eq!(
        executor::verdict_of(&[mk(Passed), mk(Passed)]),
        Verdict::Success
    );
    assert_eq!(
        executor::verdict_of(&[mk(Passed), mk(Failed)]),
        Verdict::Failure
    );
    assert_eq!(executor::verdict_of(&[mk(TimedOut)]), Verdict::Failure);
    assert_eq!(
        executor::verdict_of(&[mk(Passed), mk(FailedToStart)]),
        Verdict::Inconclusive
    );
}

// ---- export -----------------------------------------------------------------

#[test]
fn export_separates_observed_verified_human_and_unknown() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::new(data.path());
    let task = path_task(data.path(), vec![check("ok", "true", &[])]);
    let mut run = record(&store, &task);
    run.transition(RunStatus::Running).unwrap();
    run.transition(RunStatus::Evaluating).unwrap();
    run.verification.push(CheckResult {
        name: "ok".into(),
        command_line: "true".into(),
        outcome: CheckOutcome::Passed,
        exit_code: Some(0),
        duration_ms: 3,
        stdout: Default::default(),
        stderr: Default::default(),
        error: None,
    });
    run.evaluation.verified = Some(Verdict::Success);
    run.transition(RunStatus::Succeeded).unwrap();
    run.apply_judgment(Verdict::Inconclusive, "unsure".into())
        .unwrap();

    let mut in_progress = record(&store, &task);
    in_progress.transition(RunStatus::Running).unwrap();

    let text = export::export_jsonl(&[run.clone(), in_progress]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "runs in progress are not exported");
    let line: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(line["schema"], export::EXPORT_SCHEMA);
    assert_eq!(line["run_id"], run.id);
    assert_eq!(line["task"]["version"], 1);
    assert_eq!(line["task"]["digest"], task.digest());
    assert_eq!(line["final_status"], "succeeded");
    assert_eq!(line["verified"]["outcome"], "success");
    assert_eq!(line["verified"]["checks"][0]["outcome"], "passed");
    assert_eq!(line["human"]["verdict"], "inconclusive");
    assert_eq!(line["observed"]["app_version"], "9.9.9");
    let unknown: Vec<&str> = line["unknown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(unknown.contains(&"observed.model"));
    assert!(unknown.contains(&"observed.agent_version"));
    assert!(unknown.contains(&"observed.exit_code"));
    assert!(
        !unknown.contains(&"observed.cancellation"),
        "a cancellation that never happened is not unknown"
    );
    assert!(!unknown.contains(&"human"));
    for key in ["observed", "verified", "human", "agent_claims", "unknown"] {
        assert!(line.get(key).is_some(), "missing {key}");
    }
}

// ---- execution end to end with fake agents -------------------------------------

#[cfg(unix)]
mod execution {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fake stack-agent: checks it was invoked the documented way, prints a
    /// `work --json`-shaped report (leaking the key to test redaction), then
    /// runs `body`.
    fn fake_agent(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-stack-agent");
        let script = format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "stack-agent 0.1.0-fake"; exit 0; fi
[ "$1" = "work" ] || {{ echo "error: expected work" >&2; exit 2; }}
case "$*" in *"--json"*) ;; *) echo "error: expected --json" >&2; exit 2;; esac
[ -n "$CHIP_MODEL" ] || {{ echo "error: no model is selected" >&2; exit 3; }}
echo "turn 1: asking the model with $CHIP_API_KEY" >&2
{body}
"#
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    struct Harness {
        _data: tempfile::TempDir,
        project: tempfile::TempDir,
        store: Store,
        listener: std::net::TcpListener,
        events: Arc<Mutex<Vec<RunStatus>>>,
    }

    impl Harness {
        fn new() -> Self {
            let data = tempfile::tempdir().unwrap();
            let store = Store::new(data.path());
            Self {
                _data: data,
                project: tempfile::tempdir().unwrap(),
                store,
                listener: std::net::TcpListener::bind("127.0.0.1:0").unwrap(),
                events: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn api_url(&self) -> String {
            format!("http://{}/v1", self.listener.local_addr().unwrap())
        }

        async fn run(
            &self,
            task: &BenchTask,
            agent: Option<&Path>,
            cancel: CancellationToken,
        ) -> RunRecord {
            let config = RunConfig {
                agent_id: "stack-agent".into(),
                model: Some("fake-model".into()),
                api_url: Some(self.api_url()),
                api_key: Some(KEY.into()),
                executable: agent.map(|p| p.to_string_lossy().into_owned()),
            };
            let launch = adapters::resolve_executable(
                "stack-agent-that-does-not-exist",
                config.executable.as_deref(),
            )
            .and_then(|exe| adapters::plan(task, &config, &exe));
            let run = executor::new_record(&self.store, task, "stack-agent", "9.9.9");
            let events = self.events.clone();
            let ctx = Context {
                store: self.store.clone(),
                notify: Arc::new(move |r: &RunRecord| events.lock().unwrap().push(r.status)),
                cancel,
                slot: Arc::new(tokio::sync::Semaphore::new(1)),
                launch,
                path_env: std::env::var("PATH").unwrap_or_default(),
            };
            let done = executor::execute(ctx, run).await;
            assert_eq!(
                self.store.get_run(&done.id).unwrap(),
                done,
                "the final record is what was saved"
            );
            done
        }
    }

    const REPORT: &str = r#"echo "{\"command\":\"work\",\"terminal_state\":\"completed\",\"verified\":true,\"note\":\"key=$CHIP_API_KEY\",\"agent_state\":{\"task_id\":\"task_abc\"}}""#;

    fn done_check() -> Vec<VerificationCommand> {
        vec![check("done.txt exists", "test", &["-f", "done.txt"])]
    }

    #[tokio::test]
    async fn verified_success_comes_from_the_checks() {
        let h = Harness::new();
        let agent = fake_agent(
            h.project.path(),
            &format!("echo hi > done.txt\n{REPORT}\nexit 0"),
        );
        let task = path_task(h.project.path(), done_check());
        let run = h.run(&task, Some(&agent), CancellationToken::new()).await;
        assert_eq!(run.status, RunStatus::Succeeded, "{:?}", run.errors);
        let exec = run.execution.as_ref().unwrap();
        assert_eq!(exec.outcome, ExecutionOutcome::Exited);
        assert_eq!(exec.exit_code, Some(0));
        assert_eq!(run.agent.version.as_deref(), Some("stack-agent 0.1.0-fake"));
        assert_eq!(run.model.as_deref(), Some("fake-model"));
        assert_eq!(run.verification[0].outcome, CheckOutcome::Passed);
        assert!(run
            .changes
            .as_ref()
            .unwrap()
            .added
            .contains(&"done.txt".to_string()));
        assert!(run
            .artifacts
            .iter()
            .any(|a| a.reference.contains("task_abc")));
        assert_eq!(
            exec.agent_report.as_ref().unwrap()["terminal_state"],
            "completed"
        );
        // No credential anywhere in the stored record.
        let stored =
            std::fs::read_to_string(h.store.root().join("runs").join(format!("{}.json", run.id)))
                .unwrap();
        assert!(!stored.contains(KEY), "credential leaked into the ledger");
        assert!(run.credential_supplied);
        let seen = h.events.lock().unwrap().clone();
        assert!(seen.contains(&RunStatus::Running));
        assert!(seen.contains(&RunStatus::Evaluating));
        assert_eq!(seen.last(), Some(&RunStatus::Succeeded));
    }

    #[tokio::test]
    async fn exit_zero_without_the_change_is_a_verification_failure() {
        let h = Harness::new();
        let agent = fake_agent(h.project.path(), &format!("{REPORT}\nexit 0"));
        let run = h
            .run(
                &path_task(h.project.path(), done_check()),
                Some(&agent),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.failure_kind, Some(FailureKind::Verification));
        assert_eq!(
            run.execution.as_ref().unwrap().outcome,
            ExecutionOutcome::Exited,
            "the agent itself succeeded"
        );
        assert_eq!(run.evaluation.verified, Some(Verdict::Failure));
    }

    #[tokio::test]
    async fn an_agent_failure_is_recorded_apart_from_verification() {
        let h = Harness::new();
        let agent = fake_agent(
            h.project.path(),
            "echo 'error: the selected model did not answer' >&2\nexit 4",
        );
        let run = h
            .run(
                &path_task(h.project.path(), done_check()),
                Some(&agent),
                CancellationToken::new(),
            )
            .await;
        let exec = run.execution.as_ref().unwrap();
        assert_eq!(exec.outcome, ExecutionOutcome::ExitedNonZero);
        assert_eq!(exec.exit_code, Some(4));
        assert!(exec.error.as_deref().unwrap().contains("did not answer"));
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.verification.len(), 1, "verification still ran");
    }

    #[tokio::test]
    async fn a_missing_executable_fails_before_running() {
        let h = Harness::new();
        let run = h
            .run(
                &path_task(h.project.path(), done_check()),
                None,
                CancellationToken::new(),
            )
            .await;
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.failure_kind, Some(FailureKind::Execution));
        assert_eq!(
            run.execution.as_ref().unwrap().outcome,
            ExecutionOutcome::FailedToStart
        );
        assert!(run.errors[0].contains("Launch page"), "{:?}", run.errors);
        assert!(run.verification.is_empty());
    }

    #[tokio::test]
    async fn an_unreachable_endpoint_fails_with_what_to_do() {
        let h = Harness::new();
        let agent = fake_agent(h.project.path(), "exit 0");
        let task = path_task(h.project.path(), done_check());
        let port = h.listener.local_addr().unwrap().port();
        drop(h.listener);
        let config = RunConfig {
            agent_id: "stack-agent".into(),
            model: Some("m".into()),
            api_url: Some(format!("http://127.0.0.1:{port}/v1")),
            api_key: None,
            executable: Some(agent.to_string_lossy().into_owned()),
        };
        let ctx = Context {
            store: h.store.clone(),
            notify: Arc::new(|_: &RunRecord| {}),
            cancel: CancellationToken::new(),
            slot: Arc::new(tokio::sync::Semaphore::new(1)),
            launch: adapters::plan(&task, &config, &agent),
            path_env: std::env::var("PATH").unwrap_or_default(),
        };
        let run = executor::execute(
            ctx,
            executor::new_record(&h.store, &task, "stack-agent", "1"),
        )
        .await;
        assert_eq!(run.status, RunStatus::Failed);
        assert!(
            run.errors[0].contains("Local API Server"),
            "{:?}",
            run.errors
        );
    }

    #[tokio::test]
    async fn a_verification_command_that_cannot_start_is_inconclusive() {
        let h = Harness::new();
        let agent = fake_agent(h.project.path(), "exit 0");
        let task = path_task(
            h.project.path(),
            vec![check("missing", "definitely-not-a-program-xyz", &[])],
        );
        let run = h.run(&task, Some(&agent), CancellationToken::new()).await;
        assert_eq!(run.status, RunStatus::Inconclusive);
        assert_eq!(run.failure_kind, Some(FailureKind::VerificationUnavailable));
        assert_eq!(run.verification[0].outcome, CheckOutcome::FailedToStart);
    }

    #[tokio::test]
    async fn the_time_limit_stops_the_agent_and_its_children() {
        let h = Harness::new();
        let marker = h.project.path().join("child-survived");
        // The child sleeps then writes a marker; it must die with the agent.
        let agent = fake_agent(
            h.project.path(),
            &format!(
                "(sleep 4; touch '{}') &\ntrap '' TERM\nsleep 60",
                marker.display()
            ),
        );
        let task = path_task(h.project.path(), done_check());
        let config = RunConfig {
            agent_id: "stack-agent".into(),
            model: Some("m".into()),
            api_url: Some(h.api_url()),
            api_key: None,
            executable: Some(agent.to_string_lossy().into_owned()),
        };
        let mut launch = adapters::plan(&task, &config, &agent).unwrap();
        launch.timeout = Duration::from_millis(500);
        let ctx = Context {
            store: h.store.clone(),
            notify: Arc::new(|_: &RunRecord| {}),
            cancel: CancellationToken::new(),
            slot: Arc::new(tokio::sync::Semaphore::new(1)),
            launch: Ok(launch),
            path_env: std::env::var("PATH").unwrap_or_default(),
        };
        let started = std::time::Instant::now();
        let run = executor::execute(
            ctx,
            executor::new_record(&h.store, &task, "stack-agent", "1"),
        )
        .await;
        // TERM is ignored, so this waited for the grace period and killed it.
        assert!(started.elapsed() < Duration::from_secs(40));
        let exec = run.execution.as_ref().unwrap();
        assert_eq!(exec.outcome, ExecutionOutcome::TimedOut);
        assert_eq!(run.cancellation.as_ref().unwrap().reason, "timeout");
        assert_eq!(
            run.status,
            RunStatus::Failed,
            "verification still decided it"
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(!marker.exists(), "a child of the agent outlived it");
    }

    #[tokio::test]
    async fn cancellation_stops_a_running_agent() {
        let h = Harness::new();
        let agent = fake_agent(h.project.path(), "sleep 60");
        let task = path_task(h.project.path(), done_check());
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            trigger.cancel();
        });
        let started = std::time::Instant::now();
        let run = h.run(&task, Some(&agent), cancel).await;
        assert!(started.elapsed() < Duration::from_secs(15));
        assert_eq!(run.status, RunStatus::Cancelled);
        assert_eq!(run.failure_kind, Some(FailureKind::Cancelled));
        assert_eq!(
            run.execution.as_ref().unwrap().outcome,
            ExecutionOutcome::Cancelled
        );
        assert!(
            run.verification.is_empty(),
            "nothing is verified after a cancel"
        );
        assert_eq!(run.cancellation.as_ref().unwrap().reason, "user");
    }

    #[tokio::test]
    async fn a_run_cancelled_while_queued_never_starts() {
        let h = Harness::new();
        let agent = fake_agent(h.project.path(), "touch started; exit 0");
        let task = path_task(h.project.path(), done_check());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let run = h.run(&task, Some(&agent), cancel).await;
        assert_eq!(run.status, RunStatus::Cancelled);
        assert!(run.execution.is_none());
        assert!(!h.project.path().join("started").exists());
    }

    #[tokio::test]
    async fn manual_runs_wait_for_review() {
        let h = Harness::new();
        let agent = fake_agent(h.project.path(), "exit 0");
        let mut task = path_task(h.project.path(), vec![]);
        task.evaluation = Evaluation::Manual {
            instructions: String::new(),
        };
        let run = h.run(&task, Some(&agent), CancellationToken::new()).await;
        assert_eq!(run.status, RunStatus::Evaluating);
        assert!(run.evaluation.awaiting_review);
        // Survives a restart unchanged.
        let store = Store::new(h._data.path());
        assert!(store.recover_stale_runs().unwrap().is_empty());
    }

    #[tokio::test]
    async fn output_is_capped() {
        let h = Harness::new();
        let agent = fake_agent(
            h.project.path(),
            "head -c 3000000 /dev/zero | tr '\\0' 'x' >&2\nexit 0",
        );
        let run = h
            .run(
                &path_task(h.project.path(), done_check()),
                Some(&agent),
                CancellationToken::new(),
            )
            .await;
        let stderr = &run.execution.as_ref().unwrap().stderr;
        assert!(stderr.truncated);
        assert!(stderr.total_bytes >= 3_000_000);
        assert!(stderr.text.len() < super::super::process::STDERR_LIMIT + 200);
    }

    /// A real agent through the bench, opt-in (needs the agent, its model
    /// endpoint and, for stack-agent, `pax`):
    ///
    /// ```sh
    /// ATOMIC_BENCH_AGENT=stack-agent ATOMIC_BENCH_API_URL=http://127.0.0.1:1337/v1 \
    /// ATOMIC_BENCH_MODEL=<model> [ATOMIC_BENCH_TASK=ledger-fix-discount] \
    /// [ATOMIC_BENCH_EXECUTABLE=/path/to/stack-agent] \
    /// cargo test --lib agent_bench::tests::execution::live_agent_run -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "runs a real agent; see the doc comment"]
    async fn live_agent_run() {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let agent = var("ATOMIC_BENCH_AGENT").unwrap_or_else(|| "stack-agent".into());
        let task_id = var("ATOMIC_BENCH_TASK").unwrap_or_else(|| "ledger-fix-discount".into());
        let data = tempfile::tempdir().unwrap();
        let store = Store::new(data.path());
        let task = store.get_task(&task_id).unwrap();
        let config = RunConfig {
            agent_id: agent.clone(),
            model: var("ATOMIC_BENCH_MODEL"),
            api_url: var("ATOMIC_BENCH_API_URL"),
            api_key: var("ATOMIC_BENCH_API_KEY"),
            executable: var("ATOMIC_BENCH_EXECUTABLE"),
        };
        let bin = adapters::adapter(&agent).expect("known agent").bin;
        let path_env = adapters::child_path();
        let launch = adapters::resolve_executable(bin, config.executable.as_deref())
            .and_then(|exe| adapters::plan(&task, &config, &exe));
        let ctx = Context {
            store: store.clone(),
            notify: Arc::new(|r: &RunRecord| eprintln!("status: {:?}", r.status)),
            cancel: CancellationToken::new(),
            slot: Arc::new(tokio::sync::Semaphore::new(1)),
            launch,
            path_env,
        };
        let run = executor::execute(ctx, executor::new_record(&store, &task, &agent, "live")).await;
        println!("{}", serde_json::to_string_pretty(&run.summary()).unwrap());
        println!("{}", export::export_jsonl(std::slice::from_ref(&run)));
        if let Some(key) = &config.api_key {
            let stored =
                std::fs::read_to_string(store.root().join("runs").join(format!("{}.json", run.id)))
                    .unwrap();
            assert!(
                key.len() < 8 || !stored.contains(key.as_str()),
                "key leaked"
            );
        }
        assert!(run.status.is_terminal());
    }

    /// The bundled fixture end to end, with a fake agent that fixes the bug
    /// and one that only edits the test. Needs `cargo`.
    #[tokio::test]
    async fn bundled_fix_task_is_verified_by_held_out_tests() {
        if adapters::search_path("cargo", &std::env::var("PATH").unwrap_or_default()).is_none() {
            eprintln!("cargo not on PATH; skipping");
            return;
        }
        let task = suite::builtin_tasks()
            .into_iter()
            .find(|t| t.id == "ledger-fix-discount")
            .unwrap();
        let h = Harness::new();
        let fixed = include_str!("fixtures/ledger/src/pricing.rs").replace('\'', "'\\''");
        let fixer = fake_agent(
            h.project.path(),
            &format!("printf '%s' '{fixed}' > src/pricing.rs\nexit 0"),
        );
        let run = h.run(&task, Some(&fixer), CancellationToken::new()).await;
        assert_eq!(run.workdir.source, "fixture");
        assert_eq!(run.status, RunStatus::Succeeded, "{:#?}", run.verification);
        assert_eq!(
            run.changes.as_ref().unwrap().modified,
            vec!["src/pricing.rs"]
        );

        // Weakening the test does not pass: the held-out copy is restored.
        let cheater = fake_agent(
            h.project.path(),
            "sed -i.bak 's/851/850/' tests/pricing.rs; rm -f tests/pricing.rs.bak\nexit 0",
        );
        let run = h.run(&task, Some(&cheater), CancellationToken::new()).await;
        assert!(run
            .changes
            .as_ref()
            .unwrap()
            .modified
            .contains(&"tests/pricing.rs".to_string()));
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.failure_kind, Some(FailureKind::Verification));
    }
}

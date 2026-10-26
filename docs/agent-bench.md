# Agent Workshop (Agent Bench)

The Agent Workshop runs coding agents against repeatable tasks from inside Atomic Chat, verifies
each result independently of what the agent says, keeps every run in a durable ledger, explains
why a run failed as far as the evidence shows, turns a failure into a regression task, runs
regression suites and compares them, and exports the ledger for analysis:

**Run → Observe → Diagnose → Reproduce → Improve → Re-run → Verify.**

Open it from the **Server** section of the sidebar (**Agent Workshop**, route `/bench/`). Tabs:
**Runs** (tasks, run settings, run inspector, run comparison, history), **Regression suites**,
**Failures**. Desktop only.

Decision records:
[run ledger](decisions/2026-10-26-record-agent-bench-runs-in-a-file-ledger-verified-by-explicit-commands.md),
[failure analysis, baselines, regressions, suites](decisions/2026-10-26-classify-agent-failures-by-deterministic-rules-over-recorded-evidence.md).
Code: `src-tauri/src/core/agent_bench/` (Rust), `web-app/src/routes/bench/`,
`web-app/src/containers/agent-bench/`, `web-app/src/lib/agent-bench/`.

## Quick start (macOS, no remote GPU or paid service)

1. Load a local model and start the Local API Server (Server → Overview). The bench starts the
   server itself when a run needs it, but a model must be loaded.
2. Prepare stack-agent with **Prepare** in Agent Bench's *Stack-Agent runtime* panel (or
   **Install** on the Launch page). This builds the latest `main` into Atomic Chat's own cache and
   keeps it current; it needs git and [rustup](https://rustup.rs). Also put
   [`pax`](https://github.com/rkendel1/pax) 0.4.1+ on your `PATH`: stack-agent's `change` work
   completes only through a passing `pax test`. Optionally install Claude Code from the Launch
   page. `cargo` must be on your login-shell `PATH` for the bundled benchmark's checks.
3. Open **Agent Bench**, pick a ★ bundled task, tick the agents, check the model, press **Run**.
4. Open a run in **Run history** to see the command, exit status, output, verification checks,
   changed files and configuration. Tick two or more runs to compare them.
5. **Export runs** writes `<data folder>/agent_bench/exports/agent-bench-runs-<time>.jsonl`.

## Tasks — `atomic.bench.task.v1`

| Field | Meaning |
| --- | --- |
| `id` | Stable id: lowercase letters, digits, `-`, `_`, `.` |
| `version` | Starts at 1; saving changed content increments it. Saving identical content keeps it |
| `name`, `description` | For people |
| `goal` | The instruction. Every agent receives the same prompt: the goal, then `Success criteria:` and the criteria as a list |
| `workdir` | `{"kind":"fixture","fixture":"ledger"}` (fresh isolated copy per run) or `{"kind":"path","path":"/abs/dir"}` (the agent works in place; changes are kept) |
| `success_criteria` | Plain-language criteria, shown to the agent and the reviewer |
| `evaluation` | `{"method":"command","commands":[{name, program, args, timeout_secs?}], "held_out_checks"?}` or `{"method":"manual","instructions"}` |
| `limits` | `timeout_secs` (default 900) and `max_turns` (passed to agents that take one) |

A run stores the exact task it was given (`task.snapshot`), its `version` and a SHA-256 `digest` of
its content. Bundled tasks are read-only; **Duplicate** makes an editable copy.

## Running and verification

- Runs are queued and executed **one at a time**: durations stay comparable and two runs never edit
  the same project at once.
- The agent runs without a shell, stdin closed, in its own process group, with the login-shell
  `PATH`. On timeout or **Cancel** the group gets SIGTERM (stack-agent stops at its next boundary and
  reports), then SIGKILL after 20 s. Output is kept up to 1 MiB (stdout) and 256 KiB (stderr) per
  stream, start and end, with the omitted byte count recorded.
- **Command evaluation**: after the agent stops (also after a timeout), each command runs in the
  run's working directory, without a shell, with its own timeout. A check passes only on exit 0.
  Commands come from the task and are shown in the UI; nothing an agent prints is executed.
  Bundled tasks also write **held-out check files** into the workspace just before verification,
  overwriting whatever the agent left there — editing a test cannot make it pass.
- **Manual evaluation**: the run waits in `evaluating` ("Awaiting review") until someone marks it
  Success / Failure / Inconclusive with an optional note. A judgment added to a command-verified run
  is stored beside the verified outcome and never replaces it.

### Lifecycle

`queued → running → evaluating → succeeded | failed | inconclusive | cancelled`. Every change is
saved immediately and listed in `history`.

| `failure_kind` | Meaning |
| --- | --- |
| `execution` | The agent could not run (not installed, no model, endpoint unreachable, spawn error). Nothing was verified |
| `verification` | The agent ran; at least one check failed or timed out |
| `verification_unavailable` | A check could not start (e.g. `cargo` missing): outcome `inconclusive` |
| `human_judgment` | A reviewer judged a manual run unsuccessful |
| `interrupted` | Atomic Chat stopped mid-run. On the next start such runs become `inconclusive`, never `succeeded` |
| `cancelled` | Cancelled by the user (no verification runs) |

The agent's exit status and self-report are recorded (`execution.exit_code`,
`execution.agent_report`) but **never decide the outcome**.

## Agents

| Agent | Invocation | Model / endpoint | Notes |
| --- | --- | --- | --- |
| stack-agent | `stack-agent work "<prompt>" --kind change --json --print-reply --deadline-secs <timeout> [--state-db <ledger>/evidence/<run>/stack-agent-state.feltdb] [--max-turns N --max-executions 2N]` (limits capped at 50); after the run, `stack-agent agent-state show --state-db … --task-id … --json` for its trajectory | `CHIP_PROVIDER`, `CHIP_ENDPOINT` (`<Local API>/v1/chat/completions`), `CHIP_MODEL`, `CHIP_API_KEY`, `CHIP_CALL_TIMEOUT_SECS=600` — the same values the Launch page writes | Other `CHIP_*` tuning variables and `OPENAI_API_KEY`/`ANTHROPIC_API_KEY` are removed from the child environment so the record describes the whole configuration. Its durable record is referenced as `stack-agent agent-state show --task-id …` |
| Claude Code | `claude -p "<prompt>" --output-format json --permission-mode acceptEdits [--model M]` | With "Use the Local API Server": `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, `ANTHROPIC_MODEL` and the tier aliases (as the Launch page writes them); `ANTHROPIC_API_KEY` removed. Otherwise Claude Code's own configuration | `acceptEdits` allows file edits; shell commands that would need approval are refused in print mode. `env` in `~/.claude/settings.json` takes precedence over these variables. No turn limit is passed |

Not yet supported in the bench (no adapter): every other Launch-page integration. Codex
(`codex exec`), OpenCode (`opencode run`) and Goose (`goose run`) have non-interactive modes and are
candidates for the next adapters; each needs its flags verified against the installed version
first. Editor and desktop-app integrations (Zed, ZCode, VS Code, JetBrains, Xcode, OpenClaw app)
have no non-interactive path.

Actionable errors are recorded on the run: missing executable ("install it from the Launch page"),
no model selected, Local API Server unreachable (checked before the agent starts), and the agent's
own `error:` lines.

## Stack-Agent runtime

stack-agent runs (in Agent Bench and from the Launch page) use the runtime Atomic Chat manages
(`src-tauri/src/core/stack_agent_runtime`; decision:
[2026-10-26](decisions/2026-10-26-manage-the-stack-agent-runtime-from-pinned-main-commits.md)).
Nothing is installed globally, and nobody runs a build or upgrade command.

1. **Resolve.** `git ls-remote https://github.com/rkendel1/stack-agent refs/heads/main` gives a
   full commit id.
2. **Reuse.** If that commit is already cached for this platform and its executable still has the
   SHA-256 recorded at validation, it is used as is.
3. **Prepare.** In a staging directory: shallow-fetch exactly that commit and check `HEAD`. Then
   `rustup toolchain install` (the repository's `rust-toolchain.toml`) and
   `cargo build --release --locked --bin stack-agent` (the release workflow's command), with
   cargo's target directory shared across revisions. Inherited toolchain and build overrides
   (`RUSTUP_TOOLCHAIN`, `RUSTFLAGS`, `CARGO_BUILD_*`, `CARGO_PROFILE_*`, …) are removed.
4. **Validate.** `--version` must report `stack-agent X.Y.Z`, the offline `--test` must complete a
   turn, and `work --help` must offer `--kind --json --deadline-secs --max-turns --max-executions
   --print-reply --state-db`.
5. **Activate.** The staging directory is renamed into the cache, then `state.json` is rewritten
   atomically. The previous runtime (and the last three) is kept for rollback.
6. **Fall back within `main`.** If the tip of `main` fails to build or validate, up to three of its
   first-parent ancestors (never older than the active runtime) are tried, and the newest that
   passes is activated. The status stays **Update failed**, naming the tip and the error, because
   the tip itself could not be prepared.

**When it refreshes.** 20 s after start and every 6 h, plus **Refresh** in the runtime panel or
**Install** on the Launch page. Background checks start only once someone has prepared a runtime.
A failed check, fetch, build or validation keeps the active runtime and is shown. An automatic
refresh does not retry a revision that failed, or one a developer rolled back from; a manual
refresh does. Interrupted preparations leave only a staging directory, which the next refresh
removes. One refresh runs at a time across processes.

**Runtime panel (Agent Bench).**
- *Status*:
  - **Current**: the active commit is `main`, checked in the last 24 h.
  - **Update available**
  - **Refreshing**: shows the build progress.
  - **Update failed**: shows the stage and error.
  - **May be stale**: the check failed or is old; the last evidence is shown.
  - **Pinned**
  - **Unmanaged**: only a `stack-agent` on `PATH` exists; its revision is unknown.
  - **Not prepared**
- *Details*: the active revision and version, its source, `main` as last seen and when, and the
  last successful refresh.
- *Actions*: **Refresh**/**Prepare** and **Roll back**.
- *Revision for runs*: **Follow main** (default) or a cached revision to pin for a controlled
  comparison. A pinned revision is never replaced by a refresh. If it becomes unusable, runs fail
  with that error rather than switching.

**Which executable a run uses**, in order:
1. A path set on the Launch page (recorded as `custom_path`).
2. The pinned revision.
3. The active managed runtime.
4. An unmanaged `stack-agent` on `PATH` (recorded as `external`, commit unknown).

The run resolves once, before it starts, and holds a lease, so activating or pruning another
runtime never changes what a run executes. Just before launching, the executor hashes the program
it is about to start and refuses to run it if the hash differs from the recorded
`binary_sha256`. So the recorded commit and digest are those of the file that ran.

Workshop runs, regression tasks and suite runs all take this path. A **suite run** resolves the
runtime once and gives that same managed executable to every entry, so a refresh in the middle of
a suite cannot split it across revisions.

**Provenance.** Every stack-agent run records `agent.runtime`:
- `source`: `managed`, `custom_path` or `external`;
- `selection`: `follow_main`, `pinned`, `custom_path` or `external`;
- `commit`, `version`, `platform`, `binary_sha256`, `repository`, `toolchain` and `prepared_at`;
- `latest_known_main`, `latest_checked_at` and `matches_latest`.

The run detail shows these and offers **Use this revision**, which prepares the recorded commit if
needed and pins it so the run can be reproduced. That works as long as the commit, its
dependencies and the model are still available.

Claude Code and other agents keep their own installation and are not affected.

## Comparison

Tick runs in **Run history**. For each run: task outcome (and whether it was *verified*, a *human*
judgment, or *not verified*), duration, checks passed/failed, process status, change counts
(added/modified/deleted), agent + version, the Stack-Agent revision, model + provider. Anything not recorded shows
**Unavailable** with the reason on hover — no value is invented.

The first ticked run is the reference. Runs that differ from it in task or task version,
evaluation, project/fixture, model, provider, Atomic Chat version — or, for runs of the same agent,
agent version, Stack-Agent revision or configuration — are flagged **Not directly comparable** with the
differing values. The differences are grouped by what they can explain:
- **agent runtime**: agent version or Stack-Agent revision;
- **model or inference configuration**: model, provider or agent configuration;
- **task, evaluation or fixture**;
- **other execution settings**: Atomic Chat version, or a run still in progress.

A revision that is unknown (a run made before runtime management, or an unmanaged executable) is
never treated as equal to another.
Different agents are what is being compared and are not flagged. Runs of different tasks are
marked as not comparable at all.

The change summary is a content snapshot before and after the agent (no git needed); it skips
`.git`, `target`, `node_modules`, `.venv`, `__pycache__` and the agents' own bookkeeping directories
(`.stack-agent`, `.claude`), and says when a project has more than 20 000 files.

## Run inspector

Open a run from **Run history**, a suite, or the **Failures** tab. It answers, in order:

| Question | Where it comes from |
| --- | --- |
| What was the agent asked? | The exact prompt, task id, version and digest; for a regression task, the run it was made from |
| Why did it fail? | The failure analysis (below): candidate causes, each with source, rule, confidence and evidence |
| Where did execution stop? | The agent's last terminal event when its trajectory was captured, otherwise what the bench observed (not started, time limit, cancelled, process exit) |
| What remains unknown? | The analysis' unknowns, e.g. a trajectory that could not be read, or that a model's stated reason is only a claim |
| What happened, in order? | A timeline of the bench's status changes and process start/end, the agent's own events, each check, and reviews. Entries without evidence are shown as **Missing** |
| What did it execute? | Command line, process outcome, exit code/signal, stdout/stderr, the agent's report |
| What did the model say? | The replies the agent printed (stack-agent `--print-reply`), labeled as claims |
| What changed? Which checks passed? | Change summary; each check with its command, exit code and output |
| Can it be reproduced? | The baseline identity, whether the workspace was fresh, excluded paths and limitations |

Every item carries one evidence class, kept apart everywhere (inspector, timeline, analysis,
export):

| Class | Meaning |
| --- | --- |
| Observed | Atomic Chat or the agent's host saw it happen (a process exited, an execution failed) |
| Agent runtime | The agent's runtime decided it (it escalated, its loop guard stopped it) |
| Agent claim | The model said it (a decision, a reason, "done"). Never evidence of success |
| Verified | A verification command established it |
| Human | A reviewer concluded it |
| Missing | The evidence does not exist for this run |

The bench never infers hidden reasoning or invents tool calls: the timeline holds only what the
agent recorded.

## Failure analysis — `atomic.bench.analysis.v1`

Every finished run (and a manual run awaiting review) carries an analysis computed by deterministic
rules (`atomic.bench.rules.1`) from the record alone. Causes are **candidates**, never facts. A run
can have several; one is primary (a reviewer's first; then what stopped the work; then
explanations; then "insufficient evidence"; the bare fact of failed checks last).

| Rule | Category | Confidence | When |
| --- | --- | --- | --- |
| R1 | Process or infrastructure | high | Atomic Chat stopped mid-run (interrupted) |
| R2 | Process or infrastructure | high | The agent could not start |
| R3 | Timeout or resource limit | high | The task's time limit stopped the agent |
| R4 | Timeout or resource limit | high | The agent's own turn/execution/time limit stopped it |
| R5 | Model output or decision format | medium | A model reply hit its output-token cap |
| R6 | Process or infrastructure | medium | A model call failed (timeout, transport) |
| R7 | Model output or decision format | high | Replies were not valid decisions, or a decision was rejected |
| R8 | Safety or authority rejection | high | Audit violations, replaced test files, invalidated evidence, protected/reserved paths |
| R9 | Goal not completed | high | The agent escalated or was blocked, and checks failed |
| R10 | Incorrect or incomplete implementation | high | The agent claimed completion, changed files, and checks failed |
| R11 | Goal not completed | high | The agent claimed completion without changing anything, and checks failed |
| R12 | Incorrect or incomplete implementation | medium | Files changed (no completion claim) and checks failed |
| R13 | Verification failure | high | A check failed or timed out |
| R14 | Tool or capability failure | medium | A capability failed (e.g. `project.git.status: not_a_repository`); contributing only |
| R15 | Context or discovery | medium | The model's context hit its limit |
| R16 | Context or discovery | low | The agent stopped without listing, searching or reading the project |
| R17 | Process or infrastructure | medium | A verification command could not run |
| R18 | Insufficient evidence | high | Verification established no outcome |
| R19 | Insufficient evidence | high | Waiting for a reviewer |
| R20 | Insufficient evidence | high | The run failed and nothing in the evidence explains why |
| R21 | Context or discovery | medium | stack-agent's loop guard stopped it for repeating a search/read/list |

Reviewers can record a cause (category, confidence, note) in the inspector; it is stored beside
the rules' causes, which stay as they were. Diagnoses proposed by a model or tool go in
`suggestions`, which the rules never read and which never become causes. A changed rule gets a new
rules version.

## Baselines and fresh workspaces

Every run records `baseline`: what it started from.

| Workdir | Start | Identity |
| --- | --- | --- |
| `fixture` | A fresh copy of the bundled files, made a git repository with one baseline commit (fixed author, date and config, no hooks), so the same fixture always yields the same commit | `fixture:<id>@<content sha256>` |
| `git_snapshot` | A fresh copy of one commit's tracked files (`git archive`), minus credential-shaped paths (`.env*`, `.ssh`, `.aws`, private keys, `*.pem`, `secrets.*`, …), committed the same way. The repository is only read | `git:<commit>` |
| `path` | The project itself, in place | `git:<HEAD>` when clean, else `path:<content sha256>`; the record says the start cannot be restored, and whether the tree was dirty |

A fixture or snapshot workspace is never reused: a run refuses a directory that exists.

## Regression tasks

From a finished run, **Create regression task** makes a new task (new id, version 1) that copies
what the run was given — goal, criteria, verification commands, held-out checks, limits — and
records `origin`: the source run, task id/version/digest, status, checks, primary cause, agent,
version and model, baseline, a person's required description of the failure, an optional proposed
improvement, and limitations. Its `expected` outcome defaults to `success` (the fix should make
every check pass). The source task and run are only read.

A project source becomes a `git_snapshot` of its starting commit only if that start was clean;
otherwise the regression runs on the project in place and says it cannot be reproduced. Every
regression records that model output is not deterministic.

Every saved task version is archived write-once (`task_versions/<id>/v<n>.json`), so a suite can
pin a version that later edits or a deletion cannot change.

## Regression suites — `atomic.bench.suite.v1`

A suite pins tasks at a version, each with its expected outcome, verification procedure, baseline
and source run. **Run suite** runs every entry at its pinned version with one agent/model
configuration (taken from the Runs tab), one at a time, as a suite run
(`atomic.bench.suite_run.v1`, labeled e.g. "baseline" or "after-fix"). An entry whose pinned
version is no longer stored, or whose digest changed, is recorded as not started with the reason.

Aggregation never counts an unverified run either way:

- **Verified success rate** = verified successes / (verified successes + verified failures), shown
  with its denominator; no rate when nothing was verified.
- **Exclusions**, each with a reason: inconclusive, agent could not run, interrupted, cancelled,
  still running, human-judged (reported apart), not started, run missing from the ledger.
- Durations are summed over the runs that recorded one, and say how many did.

Pick a baseline and a candidate suite run to compare. A task is an **improvement** or
**regression** only if both sides verified it at the same task version and the same baseline;
otherwise it is **not comparable**, with the reason. Warnings list every condition that differs:
suite version, agent, agent version, Stack-Agent revision (`agent_revision`, same agent only),
model, provider, configuration (same agent only) and app version, and conditions recorded as
unknown, which cannot be confirmed equal. A stack-agent run with no recorded runtime (made before
runtime management, or an unmanaged executable) has an unknown revision. Other agents have no
managed runtime and are "not applicable", never unknown.

## Failures — what to fix next

The **Failures** tab groups finished, unsuccessful runs (optionally for one agent) by primary
cause, most frequent first, with contributing counts and up to five example runs with their
evidence. Each example offers **Create follow-up regression task**; add it to a suite, change the
agent yourself, and run the suite again to see whether the change helped. Nothing here modifies an
agent, model settings or code.

## Storage

`<Atomic Chat data folder>/agent_bench/`:

| Path | Content |
| --- | --- |
| `tasks/<id>.json` | Saved tasks (`atomic.bench.task.v1`). Bundled tasks are built in |
| `task_versions/<id>/v<n>.json` | Every saved task version, write-once |
| `runs/<run id>.json` | One run record each (`atomic.bench.run.v1`), with its analysis |
| `evidence/<run id>/` | What the agent recorded about the run (stack-agent's FeltDB), outside the project |
| `workspaces/<run id>/` | The fixture or snapshot copy a run worked in, kept for inspection |
| `suites/<id>.json`, `suites/versions/<id>/v<n>.json` | Suites and every suite version |
| `suite_runs/<id>.json` | Suite runs |
| `exports/*.jsonl` | Exports |

Run records written before the workshop load unchanged and are analyzed when read (nothing is
rewritten).

The stack-agent runtime cache is beside it, in `<Atomic Chat data folder>/stack-agent-runtime/`:
`state.json`, `runtimes/<arch>-<os>/<commit>/` (`bin/stack-agent` and `runtime.json`, its
manifest with the health checks), `build/` (cargo's target directory) and `staging/`.

Each record is written to a temporary file and renamed, so a crash never leaves half a record. A
damaged file is skipped (and logged), never hides the others.

**Credentials** are never stored: the key reaches the agent only through its environment; every
captured string (output, errors, the agent's report, the endpoint) is scrubbed of the key and of
credential-shaped values (`sk-…`, bearer tokens, `*_API_KEY=` / `*_TOKEN=` / `SECRET` /
`PASSWORD` assignments); config omits credential-named keys; the record says only
`credential_supplied: true|false`.

## Export — `atomic.bench.export.v1`

One JSON object per line, oldest first; runs still in progress are left out (manual runs awaiting
review are included, with `final_status: "evaluating"`).

```json
{
  "schema": "atomic.bench.export.v1",
  "run_id": "run_20261026T171945_d2f4bace",
  "run_schema": "atomic.bench.run.v1",
  "task": {"id": "ledger-fix-discount", "version": 1, "digest": "<sha256>", "name": "…", "evaluation_method": "command"},
  "final_status": "succeeded",
  "failure_kind": null,
  "terminal": true,
  "observed": {"agent_id": "stack-agent", "agent_version": "stack-agent 0.1.0", "agent_revision": "<commit>",
               "agent_runtime": {"source": "managed", "selection": "follow_main", "commit": "<commit>", "version": "0.1.0",
                                 "binary_sha256": "<sha256>", "latest_known_main": "<commit>", "matches_latest": true, "…": "…"},
               "model": "…", "provider": "atomic-chat-local-api",
               "config": {"CHIP_MODEL": "…", "kind": "change", "…": "…"}, "credential_supplied": true, "app_version": "…",
               "workdir_source": "fixture", "started_at": "…", "finished_at": "…", "duration_ms": 1423,
               "execution_outcome": "exited", "exit_code": 0, "changes": {"added": [], "modified": ["src/pricing.rs"], "deleted": []}, "…": "…"},
  "verified": {"method": "command", "outcome": "success",
               "checks": [{"name": "Pricing tests pass", "command": "cargo test --offline --test pricing", "outcome": "passed", "exit_code": 0, "duration_ms": 273}]},
  "human": null,
  "agent_claims": {"terminal_state": "completed", "verified": true, "paths_written": ["src/pricing.rs"], "usage": {"…": "…"}},
  "unknown": ["human"]
}
```

| Section | Holds |
| --- | --- |
| `observed` | Facts Atomic Chat saw: agent, version, Stack-Agent revision and runtime (`agent_revision`, `agent_runtime`; `null` and not "unknown" for other agents), method, model, provider, endpoint, config, versions, times, process outcome, exit code/signal, output sizes, changes, cancellation, interruption, errors |
| `verified` | What the verification commands established, per check; `outcome` is `null` for manual evaluation or when verification never ran |
| `human` | A reviewer's verdict, note and time, or `null` |
| `agent_claims` | The agent's own statements about its work. Not evidence |
| `unknown` | Every field whose value was not available (e.g. `observed.agent_version` when `--version` did not answer). A field that is `null` because the thing did not happen (no cancellation, no signal) is not listed |

Added within `v1` (absent from lines exported before):

| Section | Holds |
| --- | --- |
| `analysis` | The failure analysis: causes with category, source, rule, confidence, evidence (each with its evidence class), unknowns, and `suggestions` kept apart |
| `reproducibility` | `baseline`, `fresh_workspace`, the task's `origin` and `expected` outcome, and every known limitation |
| `suite` | The suite, suite version and suite run the run belongs to |
| `agent_trace` | Where the trajectory came from, whether it was available (and why not), events by provenance, each event's index/time/provenance/kind/summary, and the number of model replies (their text is in `agent_claims.model_replies`) |
| `evidence_gaps` | Evidence the run lacks: `baseline`, `agent_trace`, `model_replies`, `process_timestamps` |

New fields may be added within `v1`; a changed meaning gets a new schema version.

## Bundled benchmark — `atomic-ledger` v1

Fixture: `src-tauri/src/core/agent_bench/fixtures/ledger`, a dependency-free Rust crate (integer
cents: `money`, `pricing`, `text`). Verification is `cargo test --offline`; no network, no model
weights.

| Task | Capability | Fixture | Verification |
| --- | --- | --- | --- |
| `ledger-explain-rounding` | Understand an existing codebase: name the rounding function in `ANSWER.txt` | `ledger` | held-out `tests/bench_answer.rs`, then the whole suite |
| `ledger-fix-discount` | Fix a deterministic failing test (`apply_discount` truncates instead of rounding) | `ledger-discount-bug` | `tests/pricing.rs` restored from the held-out copy, then the whole suite |
| `ledger-add-truncate` | Bounded change preserving behavior: add `text::truncate_words` | `ledger` | held-out `tests/bench_truncate.rs`, then the whole suite |

## Tests

- Rust: `cargo test --lib agent_bench` (in `src-tauri`, with the test features `make test-rust`
  uses). Fake agent executables and a local TCP listener stand in for agents and the model; no model,
  network or installed agent is needed. Covers task serialization/versioning, persistence and
  restart recovery, lifecycle transitions, cancellation and timeouts (including the agent's child
  processes), redaction, execution vs. verification failures, manual evaluation, export, adapters,
  and the bundled fixture with held-out checks (needs `cargo`).
- Rust, workshop: `agent_bench::tests_workshop` — classification from real-shaped stack-agent
  evidence, unknown and ambiguous cases, suggestions kept apart, the timeline with partial data,
  fixture/snapshot/project baselines (determinism, no reuse, credential exclusion), regression tasks
  without touching their source, task-version archiving, suite denominators and exclusions,
  comparison warnings, the failure summary, compatibility with records written before the
  workshop, and the whole loop with fake stack-agent executables (`work` and `agent-state show`).
- Rust, runtime manager: `cargo test --lib stack_agent_runtime`. A fake source whose "builds" are
  shell scripts covers:
  - first acquisition, reuse without a rebuild, and a newer `main`;
  - build failure and fallback, health-check rejection, and tampered executables;
  - interrupted preparation, rollback, pinning, and concurrent and cross-process refreshes;
  - offline start, staleness, pruning with leases, unmanaged executables, and credentials.

  `agent_bench::tests_runtime` runs Workshop tasks through managed runtimes via the same
  `prepare_run` the app uses. It checks:
  - the recorded commit, path and executable digest are those of what ran;
  - a refresh during a run neither changes nor prunes its runtime;
  - an executable changed after resolution is never launched;
  - a pinned historical revision reproduces after `main` moves;
  - a suite run keeps one runtime, and regression tasks and suites still work;
  - unknown provenance is never comparable, and other agents are not asked for one;
  - the export keeps the Workshop sections and adds the runtime;
  - records written before runtime provenance still load.
- Web: `web-app/src/lib/agent-bench/__tests__`:
  - run comparison, including revision attribution and causes;
  - stop point, rate and command-line parsing.

  Real-browser layout suites:
  - `routes/bench/__tests__/AgentBenchPage.layout.test.tsx`;
  - `containers/agent-bench/__tests__/RunInspector.layout.test.tsx`: the inspector with complete,
    partial and missing evidence;
  - `containers/agent-bench/__tests__/RuntimePanel.layout.test.tsx`: current, stale, refreshing,
    failed, pinned and not-prepared states.
- Real runtime, opt-in: resolves, fetches, builds, validates and activates the real `main`, reuses
  it, then starts "offline" from the cache:

  ```sh
  cd src-tauri
  ATOMIC_RUNTIME_ROOT=/tmp/runtime cargo test --lib live_acquire_main -- --ignored --nocapture
  ```
- Real agent, opt-in — runs the installed agent through the same executor and prints the record and
  its export line:

  ```sh
  cd src-tauri
  ATOMIC_BENCH_AGENT=stack-agent ATOMIC_BENCH_API_URL=http://127.0.0.1:1337/v1 \
  ATOMIC_BENCH_MODEL=<loaded model> [ATOMIC_BENCH_TASK=ledger-fix-discount] \
  cargo test --lib agent_bench::tests::execution::live_agent_run -- --ignored --nocapture
  ```

  For Claude Code use `ATOMIC_BENCH_AGENT=claude-code` and the bare server URL
  (`http://127.0.0.1:1337`). `ATOMIC_BENCH_DATA_DIR=<dir>` keeps the ledger (point it at the app's
  data folder to see the runs in the app).
- The whole loop with a real agent and model, opt-in: run a task, make a regression task, add it to
  a suite, run the suite twice with the same configuration and once with another model, compare,
  export; writes inspections, comparisons and the export to `ATOMIC_BENCH_OUT`:

  ```sh
  ATOMIC_BENCH_DATA_DIR=<ledger dir> ATOMIC_BENCH_OUT=<out dir> \
  ATOMIC_BENCH_API_URL=http://127.0.0.1:1337/v1 ATOMIC_BENCH_MODEL=<model> \
  [ATOMIC_BENCH_ALT_MODEL=<another model>] \
  cargo test --lib agent_bench::tests_workshop::live_workshop_loop -- --ignored --nocapture
  ```

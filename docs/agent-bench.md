# Agent Bench

Agent Bench runs coding agents against repeatable tasks from inside Atomic Chat, verifies each
result independently of what the agent says, keeps every run in a durable ledger, compares runs,
and exports the ledger for analysis. Open it from the **Server** section of the sidebar
(**Agent Bench**, route `/bench/`). Desktop only.

Decision record: [`decisions/2026-10-26-record-agent-bench-runs-in-a-file-ledger-verified-by-explicit-commands.md`](decisions/2026-10-26-record-agent-bench-runs-in-a-file-ledger-verified-by-explicit-commands.md).
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
   page. `cargo` must be on your login-shell `PATH` for the bundled
   benchmark's checks.
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
| stack-agent | `stack-agent work "<prompt>" --kind change --json --deadline-secs <timeout> [--max-turns N --max-executions 2N]` (limits capped at 50) | `CHIP_PROVIDER`, `CHIP_ENDPOINT` (`<Local API>/v1/chat/completions`), `CHIP_MODEL`, `CHIP_API_KEY`, `CHIP_CALL_TIMEOUT_SECS=600` — the same values the Launch page writes | Other `CHIP_*` tuning variables and `OPENAI_API_KEY`/`ANTHROPIC_API_KEY` are removed from the child environment so the record describes the whole configuration. Its durable record is referenced as `stack-agent agent-state show --task-id …` |
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
runtime never changes what a run executes.

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

## Storage

`<Atomic Chat data folder>/agent_bench/`:

| Path | Content |
| --- | --- |
| `tasks/<id>.json` | Saved tasks (`atomic.bench.task.v1`). Bundled tasks are built in |
| `runs/<run id>.json` | One run record each (`atomic.bench.run.v1`) |
| `workspaces/<run id>/` | The fixture copy a run worked in, kept for inspection |
| `exports/*.jsonl` | Exports |

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
- Rust, runtime manager: `cargo test --lib stack_agent_runtime`. A fake source whose "builds" are
  shell scripts covers:
  - first acquisition, reuse without a rebuild, and a newer `main`;
  - build failure and fallback, health-check rejection, and tampered executables;
  - interrupted preparation, rollback, pinning, and concurrent and cross-process refreshes;
  - offline start, staleness, pruning with leases, unmanaged executables, and credentials.

  `agent_bench::tests::runtime_provenance` runs bench tasks through managed runtimes. It checks
  that the recorded revision is the one that ran, that a pinned historical revision reproduces after
  `main` moves, and the export.
- Web: `web-app/src/lib/agent-bench/__tests__`:
  - comparison with missing and incompatible metrics, revision attribution and causes;
  - command-line parsing.

  Real-browser layout suites:
  - `web-app/src/routes/bench/__tests__/AgentBenchPage.layout.test.tsx`;
  - `web-app/src/containers/agent-bench/__tests__/RuntimePanel.layout.test.tsx`: current, stale,
    refreshing, failed, pinned and not-prepared states.
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
  (`http://127.0.0.1:1337`).

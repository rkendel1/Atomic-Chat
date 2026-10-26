---
date: 2026-10-26
title: "Record Agent Bench runs in a file ledger, verified only by explicit commands"
---

# 2026-10-26 — Record Agent Bench runs in a file ledger, verified only by explicit commands

- **Context:** Atomic Chat could install and configure coding agents (Launch page) but not run
  one against a repeatable task, check the result, or keep evidence to compare agents and, later,
  to learn from. Desktop threads are stored as one JSON file per record under the data folder;
  SQLite is used only on mobile.
- **Decision:**
  1. **Storage.** Tasks and runs are JSON files under `<data folder>/agent_bench/{tasks,runs}`
     (`atomic.bench.task.v1`, `atomic.bench.run.v1`), written to a temp file and renamed. No
     second database. A run left non-terminal by a previous process is marked `inconclusive`
     (`failure_kind: interrupted`) on first use, never `succeeded`; a manual run awaiting review is
     left alone.
  2. **Outcome.** `succeeded` only when every verification command exits 0, or a person judges a
     manual-review run successful. Agent exit status and self-reports (stack-agent's `verified`,
     Claude Code's `result`) are stored as the agent's claims and never decide the outcome. Agent
     failure (`failure_kind: execution`) is kept apart from verification failure
     (`verification`) and from verification that could not run (`verification_unavailable`).
  3. **Commands.** Verification commands come only from the task, are shown in the UI, and run
     without a shell. Nothing an agent prints is executed.
  4. **Agents.** One small adapter per agent behind a common record: stack-agent via its documented
     `work --kind change --json --deadline-secs` with `CHIP_*` from the existing
     `stack_agent_env_vars`; Claude Code via `claude -p --output-format json --permission-mode
     acceptEdits`. No invented common CLI.
  5. **Isolation.** The bundled benchmark is a dependency-free Rust crate embedded in the app and
     copied into a fresh workspace per run; held-out check files are written over the workspace
     just before verification, so editing a test cannot pass it.
  6. **Credentials.** The key reaches the agent through its environment only; every stored string
     is scrubbed of it and of credential-shaped values, and config omits credential-named keys.
  7. **Export.** JSONL `atomic.bench.export.v1` separating `observed`, `verified`, `human`,
     `agent_claims` and an explicit `unknown` list.
- **Consequences:** Runs survive restarts and are comparable across agents, models and versions;
  the comparison flags differing task versions, models, providers, agent versions/configs and app
  versions instead of hiding them. Runs execute one at a time (a semaphore), so durations are
  comparable and two runs never edit one project at once. Claude Code may refuse shell commands
  under `acceptEdits` in print mode, and `~/.claude/settings.json` `env` can override the endpoint
  the bench passes; both are documented. Listing reads every run file: fine for thousands of runs,
  revisit if the ledger grows far beyond that. No training or self-modification is built on this.
- **Owner:** `team`
- **Links:** `src-tauri/src/core/agent_bench/`, `web-app/src/routes/bench/`,
  `web-app/src/lib/agent-bench/`, `docs/agent-bench.md`.

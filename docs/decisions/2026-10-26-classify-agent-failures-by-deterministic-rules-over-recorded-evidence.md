---
date: 2026-10-26
title: "Classify agent failures by deterministic rules over recorded evidence"
---

# 2026-10-26 — Classify agent failures by deterministic rules over recorded evidence

- **Context:** Agent Bench recorded runs and verified outcomes, but not *why* a run failed, and
  could not turn a failure into a repeatable test. Real-model runs of stack-agent 0.1.0
  (Qwen2.5-Coder 1.5B and 3B, Q4_K_M, on Atomic Chat's pinned llama.cpp b11870, CPU) showed what
  the bench missed: the trajectory existed only in stack-agent's own per-project FeltDB; the
  model's reply text was not captured; fixture copies were not git repositories, so
  `project.git.*` capabilities failed (`not_a_repository`); and the run detail showed only "exit 1"
  for runs that escalated on turn 1, were rejected for an `inputs` member on a no-input
  capability, or were stopped by stack-agent's `stuck:` loop guard.
- **Decision:**
  1. **Evidence through existing interfaces only.** The stack-agent adapter passes a per-run
     `--state-db` (in the ledger, outside the project) and `--print-reply`, and after the run
     reads `stack-agent agent-state show --json`. Each event keeps stack-agent's provenance
     (`host_observed` → observed, `model_inferred` → agent claim, `runtime_decided` → agent
     runtime). No change to stack-agent or PAX was needed. Claude Code's trajectory is recorded as
     unavailable, with the reason.
  2. **Failure analysis `atomic.bench.analysis.v1`, rules `atomic.bench.rules.1`.** Stored on each
     finished run. Deterministic rules (R1–R21) map the bench's observations, verification
     results, the adapter's normalized signals and a person's review to candidate causes in ten
     categories. Each cause names its source (`rule` or `human`), rule id, confidence and
     evidence; several can apply; one is primary (a person's first, then what stopped the work,
     then explanations, then "insufficient evidence", and the bare fact of failed checks last).
     What the evidence cannot tell is listed as unknowns. Model suggestions are a separate list
     the rules never read. No LLM judges anything.
  3. **Baselines.** Every attempt records its starting state. Fixture copies become a git
     repository with one baseline commit made with a fixed identity, date and config (the same
     fixture always yields the same commit); git snapshots copy one commit's tracked files minus
     credential-shaped paths; projects run in place and record their HEAD, cleanliness and content
     digest, with the limitation that their start cannot be restored.
  4. **Regression tasks** copy the source run's exact task version (goal, checks, held-out files,
     limits) into a new task id with an `origin` record and an `expected` outcome; the source task
     and run are only read. A project source becomes a git snapshot only if it started clean.
     Every task version is archived write-once, so suites pin versions that later edits cannot
     change.
  5. **Suites `atomic.bench.suite.v1` / runs `atomic.bench.suite_run.v1`.** The verified success
     rate counts only verified outcomes; every other run is an exclusion with its reason. A
     comparison reports improvement or regression only for tasks both sides verified at the same
     task version and baseline, and warns on differing suite version, agent version, model,
     provider, configuration (same agent) or app version, including "unknown".
  6. **Export stays `atomic.bench.export.v1`**, with added sections (`analysis`,
     `reproducibility`, `suite`, `agent_trace`, `evidence_gaps`); records written before load and
     export unchanged, analyzed on read.
- **Consequences:** The bench now answers "what failed, on what evidence, and what is unknown",
  and a failure becomes a pinned, re-runnable regression with a comparable baseline. Fixture runs
  made before this change have no baseline and different git conditions; the comparison flags
  them instead of mixing them. Rules will be wrong in ways real runs reveal: they are versioned
  so a later rules version never relabels an old analysis silently. Model output stays
  non-deterministic; every regression task records that.
- **Owner:** `team`
- **Links:** `src-tauri/src/core/agent_bench/{analysis,evidence,baseline,regression,regression_suite,timeline}.rs`,
  `web-app/src/containers/agent-bench/`, `docs/agent-bench.md`,
  [2026-10-26 run ledger](2026-10-26-record-agent-bench-runs-in-a-file-ledger-verified-by-explicit-commands.md).

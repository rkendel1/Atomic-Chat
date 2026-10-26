---
date: 2026-10-26
title: "Manage the stack-agent runtime from pinned main commits"
---

# 2026-10-26 — Manage the stack-agent runtime from pinned main commits

- **Context:** Atomic Chat ran whichever `stack-agent` came first on `PATH` (or in `~/.local/bin` or
  `~/.cargo/bin`), installed by a curl script or `cargo install --git`. That executable went stale as
  `rkendel1/stack-agent` moved on, nobody could tell which revision it was (`--version` prints only
  the crate version, `0.1.0`, for every commit), and Agent Bench records were therefore not
  attributable to a revision. Official release archives exist only for tags (`v0.1.0`, which lags
  `main`); their `SHA256SUMS` comes from the same release, so it shows integrity, not provenance
  (attestation checks need the GitHub CLI). When this was built, the real `main` (e0ca87d) and its
  parent (6631187) did not compile (E0425 in `crates/stack-agent/src/software_work.rs`, confirmed
  by building them outside the manager); d376dd8, two merges back, did. Following `main` therefore
  has to survive a broken tip.
- **Decision:** One runtime manager (`src-tauri/src/core/stack_agent_runtime`) owns the stack-agent
  executable for the Launch page and Agent Bench. Claude Code and other agents are not affected.
  1. **Resolve** `refs/heads/main` with `git ls-remote` to a full commit id. A branch name is never
     built.
  2. **Reuse** the cached runtime for that commit and platform
     (`<data>/stack-agent-runtime/runtimes/<arch>-<os>/<commit>/`), checked against the SHA-256
     recorded at validation.
  3. **Prepare** a new commit in a staging directory: shallow fetch of exactly that commit, check
     `HEAD`, then `rustup toolchain install` (from the repository's `rust-toolchain.toml`) and
     `cargo build --release --locked --bin stack-agent`, the command its release workflow uses.
     Cargo's target directory is shared across revisions, so a new revision rebuilds only what
     changed. Inherited toolchain and build overrides (`RUSTUP_TOOLCHAIN`, which `cargo` sets
     for everything it runs, `RUSTFLAGS`, `CARGO_BUILD_*`, `CARGO_PROFILE_*`, …) are removed, so
     the repository's own toolchain file and profile decide the build.
  4. **Validate**: `--version` reports `stack-agent X.Y.Z`; the offline `--test` completes a turn;
     `work --help` offers every flag Atomic Chat passes. Checks run with a scratch `HOME` and without
     `CHIP_*` or `*_API_KEY` variables.
  5. **Activate**: rename the staging directory into the cache, then rewrite `state.json`
     atomically. The previous runtime is kept, along with the last three for rollback.
  6. **Record**: every stack-agent run stores `agent.runtime`: source, selection, commit, version,
     executable digest, toolchain, and `main` as last seen. Agent Bench exports it as
     `observed.agent_revision` / `observed.agent_runtime`. Runs resolve once and lease the runtime,
     so a later activation or prune never changes what a run executes.
  7. **Recover**: a failed check, fetch, build or validation leaves the active runtime in place and
     is shown. When `main`'s tip fails to build or validate, up to three of its first-parent
     ancestors (never older than the active runtime) are tried, and the newest that passes is
     activated: the latest *compatible* revision of `main`. The status still says the update
     failed, because the tip itself could not be prepared. An automatic refresh does not retry a failed revision or re-activate one a developer
     rolled back from; a manual refresh does. A pinned revision is never replaced, and if it becomes
     unusable that is an error, not a fallback. One refresh runs at a time (an in-process lock plus
     a lock file naming its process).
  The background check runs 20 s after start and every 6 h, only after someone has first prepared
  a runtime (Launch page Install, or Prepare in Agent Bench), so users who never use stack-agent
  never build it. "Current" is shown only when the active commit equals `main` as checked within
  the last 24 h. Otherwise the status says "may be stale" and gives the last evidence. An
  unmanaged `stack-agent` on `PATH` is used only when no managed runtime exists, and is labelled
  with its revision unknown. A path set on the Launch page still wins and is labelled as such.
- **Consequences:**
  - Developers no longer build, install or upgrade stack-agent by hand. The first preparation
    needs git, rustup and a few minutes of build time; later starts reuse the cache, including
    offline.
  - The build directory under the cache can grow to a few GB.
  - Comparisons attribute differences to the agent runtime (version or revision), the model or
    inference configuration, the task or fixture, or other execution settings. An unknown revision
    is never treated as equal to a known one.
  - A historical revision can be prepared from its recorded commit and pinned to reproduce a run,
    as long as the commit, its dependencies and the model remain available.
  - Release archives are not used, because they never match `main`. If stack-agent later publishes
    attested per-commit builds, `source.rs` can prefer them behind the same validation.
  - No change to stack-agent was needed: the manager knows the commit because it built it.
    `stack-agent --version` still does not report a commit, so an unmanaged executable's revision
    stays unknown.
- **Owner:** `team`
- **Links:** `src-tauri/src/core/stack_agent_runtime/`, `src-tauri/src/core/agent_bench/commands.rs`,
  `web-app/src/containers/agent-bench/RuntimePanel.tsx`, `docs/agent-bench.md`,
  [2026-10-26 Agent Bench ledger](2026-10-26-record-agent-bench-runs-in-a-file-ledger-verified-by-explicit-commands.md).

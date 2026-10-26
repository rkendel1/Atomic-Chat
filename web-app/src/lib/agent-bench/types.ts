/**
 * Agent Bench records as the Rust ledger stores them
 * (`src-tauri/src/core/agent_bench`). Field names are the on-disk snake_case.
 */

export const TASK_SCHEMA = 'atomic.bench.task.v1'

export type WorkdirRef =
  | { kind: 'fixture'; fixture: string }
  | { kind: 'path'; path: string }

export type VerificationCommand = {
  name: string
  program: string
  args: string[]
  timeout_secs?: number
}

export type Evaluation =
  | {
      method: 'command'
      commands: VerificationCommand[]
      held_out_checks?: string
    }
  | { method: 'manual'; instructions: string }

export type BenchTask = {
  schema: string
  id: string
  version: number
  name: string
  description: string
  goal: string
  workdir: WorkdirRef
  success_criteria: string[]
  evaluation: Evaluation
  limits: { timeout_secs?: number; max_turns?: number }
  builtin: boolean
  created_at: string
  updated_at: string
}

export type RunStatus =
  | 'queued'
  | 'running'
  | 'evaluating'
  | 'succeeded'
  | 'failed'
  | 'inconclusive'
  | 'cancelled'

export type FailureKind =
  | 'execution'
  | 'verification'
  | 'verification_unavailable'
  | 'human_judgment'
  | 'interrupted'
  | 'cancelled'

export type ExecutionOutcome =
  | 'exited'
  | 'exited_non_zero'
  | 'timed_out'
  | 'cancelled'
  | 'failed_to_start'

export type CheckOutcome =
  | 'passed'
  | 'failed'
  | 'timed_out'
  | 'failed_to_start'
  | 'skipped'

export type Verdict = 'success' | 'failure' | 'inconclusive'

export type CapturedText = {
  text: string
  total_bytes: number
  truncated: boolean
}

export type ExecutionRecord = {
  outcome: ExecutionOutcome
  command_line: string
  exit_code: number | null
  signal?: number
  duration_ms: number
  stdout: CapturedText
  stderr: CapturedText
  agent_report?: Record<string, unknown>
  error?: string
}

export type CheckResult = {
  name: string
  command_line: string
  outcome: CheckOutcome
  exit_code: number | null
  duration_ms: number
  stdout: CapturedText
  stderr: CapturedText
  error?: string
}

export type RunRecord = {
  schema: string
  id: string
  task: {
    id: string
    version: number
    digest: string
    name: string
    snapshot: BenchTask
  }
  agent: {
    id: string
    name: string
    execution_method: string
    executable: string | null
    version: string | null
    /** stack-agent only: the runtime the runtime manager handed this run. */
    runtime?: RuntimeProvenance | null
  }
  model: string | null
  provider: string | null
  endpoint: string | null
  config: Record<string, string>
  credential_supplied: boolean
  app_version: string
  workdir: { source: 'fixture' | 'path'; path: string; fixture: string | null }
  status: RunStatus
  failure_kind: FailureKind | null
  created_at: string
  started_at: string | null
  finished_at: string | null
  duration_ms: number | null
  execution: ExecutionRecord | null
  verification: CheckResult[]
  changes: {
    added: string[]
    modified: string[]
    deleted: string[]
    incomplete: boolean
  } | null
  evaluation: {
    method: 'command' | 'manual'
    verified: Verdict | null
    human: { verdict: Verdict; note: string; judged_at: string } | null
    awaiting_review: boolean
  }
  artifacts: { kind: string; reference: string; description: string }[]
  errors: string[]
  cancellation: { requested_at: string; reason: string } | null
  interruption: { detected_at: string; previous_status: RunStatus } | null
  history: { status: RunStatus; at: string }[]
}

export type AdapterInfo = {
  id: string
  name: string
  bin: string
  execution_method: string
  requires_model: boolean
  requires_endpoint: boolean
  notes: string
}

export type BenchOverview = {
  adapters: AdapterInfo[]
  fixtures: string[]
  suite_id: string
  suite_version: number
  data_dir: string
  app_version: string
}

export type RunConfig = {
  agent_id: string
  model: string | null
  api_url: string | null
  api_key: string | null
  executable: string | null
}

export const TERMINAL_STATUSES: RunStatus[] = [
  'succeeded',
  'failed',
  'inconclusive',
  'cancelled',
]

export const isTerminal = (status: RunStatus): boolean =>
  TERMINAL_STATUSES.includes(status)

/** How a stack-agent run's executable was chosen, and what it is. */
export type RuntimeProvenance = {
  source: 'managed' | 'custom_path' | 'external'
  selection: 'follow_main' | 'pinned' | 'custom_path' | 'external'
  path: string
  commit: string | null
  version: string | null
  platform: string
  binary_sha256: string | null
  repository: string | null
  toolchain: string | null
  prepared_at: string | null
  latest_known_main: string | null
  latest_checked_at: string | null
  matches_latest: boolean | null
}

export type RuntimeSelection =
  | { mode: 'follow_main' }
  | { mode: 'pinned'; commit: string }

export type RuntimeStatusKind =
  | 'not_installed'
  | 'current'
  | 'update_available'
  | 'refreshing'
  | 'update_failed'
  | 'stale'
  | 'pinned'
  | 'external'

export type RuntimeHealthCheck = { name: string; passed: boolean; detail: string }

export type RuntimeManifest = {
  schema: string
  commit: string
  platform: string
  repository: string
  source: string
  version: string
  binary: string
  binary_sha256: string
  toolchain: string | null
  prepared_at: string
  validated_at: string
  checks: RuntimeHealthCheck[]
}

export type RuntimeStatus = {
  status: RuntimeStatusKind
  detail: string
  enabled: boolean
  platform: string
  repository: string
  tracked_ref: string
  selection: RuntimeSelection
  resolved: RuntimeProvenance | null
  resolve_error?: string
  active: string | null
  previous: string[]
  latest_remote: { commit: string; checked_at: string } | null
  last_check: { at: string; ok: boolean; error: string | null } | null
  last_success_refresh_at: string | null
  refreshing: {
    commit: string | null
    stage: string
    started_at: string
    last_line?: string
  } | null
  failure: { commit: string; at: string; stage: string; error: string } | null
  rolled_back: string[]
  cached: {
    commit: string
    path: string
    valid: boolean
    problem?: string
    manifest?: RuntimeManifest
  }[]
}

export const shortCommit = (commit: string | null | undefined): string =>
  commit ? commit.slice(0, 12) : 'unknown'

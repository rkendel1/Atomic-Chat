/**
 * Agent Bench records as the Rust ledger stores them
 * (`src-tauri/src/core/agent_bench`). Field names are the on-disk snake_case.
 */

export const TASK_SCHEMA = 'atomic.bench.task.v1'

export type WorkdirRef =
  | { kind: 'fixture'; fixture: string }
  | { kind: 'path'; path: string }
  | { kind: 'git_snapshot'; repo: string; commit: string }

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
  /** Where a regression task came from (absent on other tasks). */
  origin?: TaskOrigin
  expected?: ExpectedOutcome
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

// ---- Agent Workshop ------------------------------------------------------

export type TaskOrigin = {
  kind: string
  source_run_id: string
  source_task_id: string
  source_task_version: number
  source_task_digest: string
  source_status: string
  source_verified: string | null
  source_checks: { name: string; outcome: string }[]
  source_primary_cause: string | null
  source_agent: string
  source_agent_version: string | null
  source_model: string | null
  source_baseline: string | null
  failure_description: string
  proposed_improvement: string
  created_at: string
  limitations: string[]
}

export type ExpectedOutcome = { outcome: 'success' | 'failure'; note: string }

export type Baseline = {
  kind: 'fixture' | 'git_snapshot' | 'path'
  identity: string
  content_digest: string
  file_count: number
  git_commit: string | null
  git_dirty: boolean | null
  source_commit: string | null
  fresh_workspace: boolean
  excluded_paths: string[]
  limitations: string[]
}

export type Provenance = 'observed' | 'agent_claim' | 'runtime'

export type TraceEvent = {
  index: number
  at: string | null
  provenance: Provenance
  kind: string
  summary: string
}

export type AgentTrace = {
  source: string
  available: boolean
  unavailable_reason: string | null
  events: TraceEvent[]
  replies: { position: number; text: string; truncated: boolean }[]
  replies_unavailable_reason: string | null
}

export type Category =
  | 'process_or_infrastructure'
  | 'timeout_or_limit'
  | 'safety_or_authority'
  | 'decision_format'
  | 'goal_not_completed'
  | 'incorrect_implementation'
  | 'tool_or_capability'
  | 'context_or_discovery'
  | 'insufficient_evidence'
  | 'verification_failure'

export const CATEGORIES: Category[] = [
  'process_or_infrastructure',
  'timeout_or_limit',
  'safety_or_authority',
  'decision_format',
  'goal_not_completed',
  'incorrect_implementation',
  'tool_or_capability',
  'context_or_discovery',
  'insufficient_evidence',
  'verification_failure',
]

export type Confidence = 'high' | 'medium' | 'low'

export type EvidenceClass =
  | 'observed'
  | 'agent_runtime'
  | 'agent_claim'
  | 'verified'
  | 'human'

export type Cause = {
  category: Category
  source: 'rule' | 'human'
  rule: string | null
  confidence: Confidence
  primary: boolean
  explanation: string
  evidence: { class: EvidenceClass; source: string; detail: string }[]
  recorded_at: string | null
}

export type FailureAnalysis = {
  schema: string
  rules_version: string
  analyzed_at: string
  status: RunStatus
  verified: Verdict | null
  summary: string
  causes: Cause[]
  unknowns: string[]
  suggestions: {
    source: string
    category: Category | null
    text: string
    created_at: string
  }[]
}

export type TimelineClass = EvidenceClass | 'missing'

/** Every evidence class, in the order the inspector's legend lists them. */
export const EVIDENCE_CLASSES: TimelineClass[] = [
  'observed',
  'agent_runtime',
  'agent_claim',
  'verified',
  'human',
  'missing',
]

export type TimelineEntry = {
  at: string | null
  class: TimelineClass
  origin: 'bench' | 'agent' | 'verification' | 'review'
  label: string
  detail: string | null
  reference: string
}

export type Signal = {
  kind: { type: string; value?: string }
  provenance: Provenance
  source: string
  detail: string
}

/** A run record with the fields the workshop added (absent on older runs). */
export type WorkshopRun = RunRecord & {
  baseline?: Baseline
  agent_trace?: AgentTrace
  analysis?: FailureAnalysis
  suite?: { suite_id: string; suite_version: number; suite_run_id: string }
}

export type RunInspection = {
  run: WorkshopRun
  prompt: string
  timeline: TimelineEntry[]
  signals: Signal[]
}

export type SuiteEntry = {
  task_id: string
  task_version: number
  task_digest: string
  task_name: string
  expected: string
  verification: string[]
  baseline: string
  source_run_id: string | null
  added_at: string
}

export type Suite = {
  schema: string
  id: string
  name: string
  description: string
  version: number
  entries: SuiteEntry[]
  created_at: string
  updated_at: string
}

export type ResultClass =
  | 'verified_success'
  | 'verified_failure'
  | 'inconclusive'
  | 'execution_failure'
  | 'interrupted'
  | 'cancelled'
  | 'in_progress'
  | 'human_judged'
  | 'not_started'

export type SuiteRun = {
  schema: string
  id: string
  suite_id: string
  suite_version: number
  label: string
  agent_id: string
  model: string | null
  entries: {
    task_id: string
    task_version: number
    expected: string
    run_id: string | null
    not_started_reason: string | null
  }[]
  created_at: string
}

export type SuiteRunSummary = {
  suite_run_id: string
  entries: {
    task_id: string
    task_version: number
    expected: string
    run_id: string | null
    class: ResultClass
    met_expectation: boolean | null
    duration_ms: number | null
    primary_cause: Category | null
    baseline: string | null
    note: string | null
  }[]
  total: number
  verified_successes: number
  verified_failures: number
  denominator: number
  verified_success_rate: number | null
  exclusions: { task_id: string; reason: string }[]
  counts: Record<string, number>
  met_expectation: number
  total_duration_ms: number
  durations_recorded: number
}

export type SuiteRunView = { suite_run: SuiteRun; summary: SuiteRunSummary }

export type Change =
  | 'improvement'
  | 'regression'
  | 'still_succeeds'
  | 'still_fails'
  | 'not_comparable'

export type SuiteComparison = {
  baseline: SuiteRunSummary
  candidate: SuiteRunSummary
  tasks: {
    task_id: string
    baseline: ResultClass | null
    candidate: ResultClass | null
    change: Change
    reason: string | null
    baseline_duration_ms: number | null
    candidate_duration_ms: number | null
  }[]
  improvements: number
  regressions: number
  not_comparable: number
  warnings: { field: string; baseline: string; candidate: string }[]
  directly_comparable: boolean
}

export type FailureSummary = {
  runs_considered: number
  succeeded: number
  not_succeeded: number
  excluded: number
  categories: {
    category: Category
    label: string
    primary_count: number
    contributing_count: number
    examples: {
      run_id: string
      /** Whether this category is the run's primary cause. */
      primary: boolean
      task_id: string
      task_version: number
      agent_id: string
      model: string | null
      source: 'rule' | 'human'
      confidence: Confidence
      explanation: string
      evidence: string[]
    }[]
  }[]
}

export type RegressionRequest = {
  id: string
  name: string | null
  failure_description: string
  proposed_improvement: string | null
  expected_outcome: 'success' | 'failure' | null
}

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

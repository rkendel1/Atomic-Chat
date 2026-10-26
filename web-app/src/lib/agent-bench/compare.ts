import type { RunRecord } from './types'
import { isTerminal } from './types'

/** A value that was observed, or why there is none. Never a made-up default. */
export type Metric<T> =
  | { available: true; value: T }
  | { available: false; reason: string }

const ok = <T>(value: T): Metric<T> => ({ available: true, value })
const missing = <T>(reason: string): Metric<T> => ({ available: false, reason })

export type Outcome = 'success' | 'failure' | 'inconclusive' | 'cancelled'

export type ComparisonRow = {
  runId: string
  agentId: string
  agentName: string
  /** The outcome and what established it. */
  outcome: Metric<{ value: Outcome; source: 'verified' | 'human' | 'none' }>
  duration: Metric<number>
  checks: Metric<{ passed: number; failed: number; total: number }>
  process: Metric<{ exitCode: number | null; outcome: string; error?: string }>
  changes: Metric<{
    added: number
    modified: number
    deleted: number
    incomplete: boolean
  }>
  agentVersion: Metric<string>
  /** The stack-agent commit that ran, when the runtime manager recorded it. */
  agentRevision: Metric<string>
  model: Metric<string>
  provider: Metric<string>
  /** Fields in which this run differs from the first one compared. */
  differences: ComparabilityField[]
}

export type ComparabilityField =
  | 'task'
  | 'task_version'
  | 'evaluation'
  | 'workdir'
  | 'agent_version'
  | 'agent_revision'
  | 'model'
  | 'provider'
  | 'config'
  | 'app_version'
  | 'in_progress'

/**
 * What a difference can explain, so a changed result is attributed to the
 * right cause: the agent's own runtime, the model or inference settings, the
 * task or its fixture, or anything else about how the run was executed.
 */
export type DifferenceCause = 'agent_runtime' | 'model' | 'task' | 'execution'

export const CAUSE_OF: Record<ComparabilityField, DifferenceCause> = {
  agent_version: 'agent_runtime',
  agent_revision: 'agent_runtime',
  model: 'model',
  provider: 'model',
  config: 'model',
  task: 'task',
  task_version: 'task',
  evaluation: 'task',
  workdir: 'task',
  app_version: 'execution',
  in_progress: 'execution',
}

export const CAUSE_ORDER: DifferenceCause[] = [
  'agent_runtime',
  'model',
  'task',
  'execution',
]

export type ComparabilityIssue = {
  field: ComparabilityField
  cause: DifferenceCause
  /** Distinct values seen, `unknown` where a run did not record one. */
  values: string[]
}

export type Comparison = {
  rows: ComparisonRow[]
  issues: ComparabilityIssue[]
  /** False when the runs did different tasks: the rows are not comparable. */
  sameTask: boolean
  /** True only when nothing but the agent differs. */
  directlyComparable: boolean
}

function outcomeOf(run: RunRecord): ComparisonRow['outcome'] {
  if (!isTerminal(run.status)) return missing('run has not finished')
  if (run.status === 'cancelled')
    return ok({ value: 'cancelled', source: 'none' })
  if (run.evaluation.method === 'manual') {
    const human = run.evaluation.human
    return human
      ? ok({ value: human.verdict, source: 'human' })
      : missing('waiting for review')
  }
  if (run.evaluation.verified)
    return ok({ value: run.evaluation.verified, source: 'verified' })
  // Failed before verification (agent could not run, app interrupted).
  return ok({
    value: run.status === 'failed' ? 'failure' : 'inconclusive',
    source: 'none',
  })
}

function checksOf(run: RunRecord): ComparisonRow['checks'] {
  if (run.evaluation.method === 'manual')
    return missing('manual evaluation has no checks')
  if (run.verification.length === 0) return missing('verification did not run')
  const passed = run.verification.filter((c) => c.outcome === 'passed').length
  const failed = run.verification.filter(
    (c) => c.outcome === 'failed' || c.outcome === 'timed_out'
  ).length
  return ok({ passed, failed, total: run.verification.length })
}

function processOf(run: RunRecord): ComparisonRow['process'] {
  const e = run.execution
  if (!e) return missing('agent did not run')
  return ok({
    exitCode: e.exit_code,
    outcome: e.outcome,
    error: e.error ?? undefined,
  })
}

function changesOf(run: RunRecord): ComparisonRow['changes'] {
  const c = run.changes
  if (!c) return missing('no change summary was recorded')
  return ok({
    added: c.added.length,
    modified: c.modified.length,
    deleted: c.deleted.length,
    incomplete: c.incomplete,
  })
}

const str = (v: string | null | undefined, reason: string): Metric<string> =>
  v ? ok(v) : missing(reason)

const configKey = (run: RunRecord) =>
  JSON.stringify(
    Object.keys(run.config)
      .sort()
      .map((k) => [k, run.config[k]])
  )

/** The commit, or `unknown`; only stack-agent runs record one. */
const revisionOf = (run: RunRecord): string =>
  run.agent.runtime?.commit ?? 'unknown'

const evaluationKey = (run: RunRecord) =>
  JSON.stringify(run.task.snapshot?.evaluation ?? null)

const workdirKey = (run: RunRecord) =>
  run.workdir.source === 'fixture'
    ? `fixture:${run.workdir.fixture}`
    : `path:${run.workdir.path}`

/**
 * How each run differs from the reference (the first run). Agents are the
 * thing being compared, so a different agent is not a difference; settings
 * that belong to one agent (its version, its config) are compared only
 * between runs of the same agent.
 */
function differencesFrom(ref: RunRecord, run: RunRecord): ComparabilityField[] {
  const out: ComparabilityField[] = []
  if (run.task.id !== ref.task.id) out.push('task')
  else if (
    run.task.version !== ref.task.version ||
    run.task.digest !== ref.task.digest
  )
    out.push('task_version')
  if (evaluationKey(run) !== evaluationKey(ref)) out.push('evaluation')
  // Every fixture run has its own workspace copy; the fixture is what counts.
  if (workdirKey(run) !== workdirKey(ref)) out.push('workdir')
  if ((run.model ?? '') !== (ref.model ?? '')) out.push('model')
  if ((run.provider ?? '') !== (ref.provider ?? '')) out.push('provider')
  if (run.agent.id === ref.agent.id) {
    if ((run.agent.version ?? '') !== (ref.agent.version ?? ''))
      out.push('agent_version')
    // An unknown revision is never assumed to match a known one.
    if (
      run !== ref &&
      run.agent.id === 'stack-agent' &&
      (revisionOf(run) !== revisionOf(ref) || revisionOf(run) === 'unknown')
    )
      out.push('agent_revision')
    if (configKey(run) !== configKey(ref)) out.push('config')
  }
  if (run.app_version !== ref.app_version) out.push('app_version')
  if (!isTerminal(run.status)) out.push('in_progress')
  return [...new Set(out)]
}

function issueValues(runs: RunRecord[], field: ComparabilityField): string[] {
  const pick = (r: RunRecord): string => {
    switch (field) {
      case 'task':
        return r.task.id
      case 'task_version':
        return `${r.task.id} v${r.task.version}`
      case 'evaluation':
        return r.evaluation.method
      case 'workdir':
        return workdirKey(r)
      case 'agent_version':
        return `${r.agent.name}: ${r.agent.version ?? 'unknown'}`
      case 'agent_revision':
        return `${r.agent.name}: ${revisionOf(r).slice(0, 12)}`
      case 'model':
        return r.model ?? 'unknown'
      case 'provider':
        return r.provider ?? 'unknown'
      case 'config':
        return `${r.agent.name}: ${Object.entries(r.config)
          .map(([k, v]) => `${k}=${v}`)
          .join(', ')}`
      case 'app_version':
        return r.app_version
      case 'in_progress':
        return r.status
    }
  }
  return [...new Set(runs.map(pick))]
}

export function compareRuns(runs: RunRecord[]): Comparison {
  if (runs.length === 0)
    return { rows: [], issues: [], sameTask: true, directlyComparable: true }
  const ref = runs[0]
  const rows: ComparisonRow[] = runs.map((run) => ({
    runId: run.id,
    agentId: run.agent.id,
    agentName: run.agent.name,
    outcome: outcomeOf(run),
    duration:
      run.duration_ms != null
        ? ok(run.duration_ms)
        : missing('run has not finished'),
    checks: checksOf(run),
    process: processOf(run),
    changes: changesOf(run),
    agentVersion: str(run.agent.version, 'version not reported'),
    agentRevision:
      run.agent.id !== 'stack-agent'
        ? missing('not a stack-agent run')
        : str(
            run.agent.runtime?.commit,
            run.agent.runtime
              ? 'unmanaged executable: revision unknown'
              : 'runtime not recorded'
          ),
    model: str(run.model, 'model not recorded'),
    provider: str(run.provider, 'provider not recorded'),
    differences: differencesFrom(ref, run),
  }))
  const fields = [...new Set(rows.flatMap((r) => r.differences))]
  const issues = fields
    .map((field) => ({
      field,
      cause: CAUSE_OF[field],
      values: issueValues(runs, field),
    }))
    .sort(
      (a, b) => CAUSE_ORDER.indexOf(a.cause) - CAUSE_ORDER.indexOf(b.cause)
    )
  return {
    rows,
    issues,
    sameTask: !fields.includes('task'),
    directlyComparable: issues.length === 0,
  }
}

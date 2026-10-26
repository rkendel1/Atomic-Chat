import { describe, expect, it } from 'vitest'
import { compareRuns } from '../compare'
import type { BenchTask } from '../types'
import { REVISION_A, run, runtime, task } from './fixtures'

describe('compareRuns', () => {
  it('reports the metrics of each run', () => {
    const { rows, directlyComparable } = compareRuns([run()])
    expect(directlyComparable).toBe(true)
    const [row] = rows
    expect(row.outcome).toEqual({
      available: true,
      value: { value: 'success', source: 'verified' },
    })
    expect(row.duration).toEqual({ available: true, value: 60000 })
    expect(row.checks).toEqual({
      available: true,
      value: { passed: 1, failed: 0, total: 1 },
    })
    expect(row.changes).toMatchObject({
      available: true,
      value: { modified: 1 },
    })
  })

  it('shows missing metrics as unavailable instead of inventing values', () => {
    const failedToStart = run({
      id: 'run_b',
      status: 'failed',
      failure_kind: 'execution',
      execution: null,
      verification: [],
      changes: null,
      model: null,
      evaluation: {
        method: 'command',
        verified: null,
        human: null,
        awaiting_review: false,
      },
      agent: { ...run().agent, version: null },
    })
    const [, row] = compareRuns([run(), failedToStart]).rows
    expect(row.process.available).toBe(false)
    expect(row.checks.available).toBe(false)
    expect(row.changes.available).toBe(false)
    expect(row.agentVersion.available).toBe(false)
    expect(row.model.available).toBe(false)
    expect(row.outcome).toEqual({
      available: true,
      value: { value: 'failure', source: 'none' },
    })
  })

  it('does not report an outcome for a run still in progress', () => {
    const live = run({ id: 'run_c', status: 'running', duration_ms: null })
    const cmp = compareRuns([run(), live])
    expect(cmp.rows[1].outcome.available).toBe(false)
    expect(cmp.rows[1].duration.available).toBe(false)
    expect(cmp.rows[1].differences).toContain('in_progress')
    expect(cmp.directlyComparable).toBe(false)
  })

  it('uses the human judgment for manual runs and nothing before it', () => {
    const manualTask: BenchTask = {
      ...task,
      evaluation: { method: 'manual', instructions: '' },
    }
    const waiting = run({
      status: 'evaluating',
      task: { ...run().task, snapshot: manualTask },
      verification: [],
      evaluation: {
        method: 'manual',
        verified: null,
        human: null,
        awaiting_review: true,
      },
    })
    expect(compareRuns([waiting]).rows[0].outcome.available).toBe(false)
    const judged = run({
      status: 'failed',
      task: { ...run().task, snapshot: manualTask },
      verification: [],
      evaluation: {
        method: 'manual',
        verified: null,
        human: { verdict: 'failure', note: '', judged_at: '' },
        awaiting_review: false,
      },
    })
    const row = compareRuns([judged]).rows[0]
    expect(row.outcome).toEqual({
      available: true,
      value: { value: 'failure', source: 'human' },
    })
    expect(row.checks.available).toBe(false)
  })

  it('compares agents without flagging the agent itself', () => {
    const claude = run({
      id: 'run_d',
      agent: {
        id: 'claude-code',
        name: 'Claude Code',
        execution_method: 'cli:claude -p',
        executable: '/bin/claude',
        version: '2.1.0',
      },
      config: { permission_mode: 'acceptEdits' },
      workdir: {
        source: 'fixture',
        path: '/data/ws/run_d',
        fixture: 'ledger-discount-bug',
      },
    })
    const cmp = compareRuns([run(), claude])
    expect(cmp.issues).toEqual([])
    expect(cmp.directlyComparable).toBe(true)
  })

  it('flags runs that are not directly comparable', () => {
    const other = run({
      id: 'run_e',
      model: 'llama-3',
      app_version: '2.3.0',
      config: { kind: 'change', max_turns: '4' },
      agent: { ...run().agent, version: 'stack-agent 0.2.0' },
      task: { ...run().task, version: 2, digest: 'd2' },
    })
    const cmp = compareRuns([run(), other])
    expect(cmp.directlyComparable).toBe(false)
    expect(cmp.sameTask).toBe(true)
    const fields = cmp.issues.map((i) => i.field).sort()
    expect(fields).toEqual(
      ['agent_version', 'app_version', 'config', 'model', 'task_version'].sort()
    )
    expect(cmp.issues.find((i) => i.field === 'model')?.values).toEqual([
      'qwen3-8b',
      'llama-3',
    ])
    expect(cmp.rows[0].differences).toEqual([])
  })

  it('marks runs of different tasks as not the same task', () => {
    const different = run({
      id: 'run_f',
      task: { ...run().task, id: 'ledger-add-truncate' },
      workdir: { source: 'fixture', path: '/x', fixture: 'ledger' },
    })
    const cmp = compareRuns([run(), different])
    expect(cmp.sameTask).toBe(false)
    expect(cmp.issues.map((i) => i.field)).toContain('workdir')
  })

  it('records an unknown model as unknown in the issue values', () => {
    const unknown = run({ id: 'run_g', model: null })
    const issue = compareRuns([run(), unknown]).issues.find(
      (i) => i.field === 'model'
    )
    expect(issue?.values).toEqual(['qwen3-8b', 'unknown'])
  })

  it('handles an empty selection', () => {
    expect(compareRuns([])).toEqual({
      rows: [],
      issues: [],
      sameTask: true,
      directlyComparable: true,
    })
  })
})

describe('compareRuns: what a difference is attributed to', () => {
  const B = '4850da9f0000000000000000000000000000beef'

  it('attributes a changed Stack-Agent revision to the agent runtime', () => {
    const newer = run({
      id: 'run_b',
      agent: { ...run().agent, runtime: runtime({ commit: B }) },
    })
    const cmp = compareRuns([run(), newer])
    expect(cmp.rows[1].differences).toEqual(['agent_revision'])
    expect(cmp.issues).toEqual([
      {
        field: 'agent_revision',
        cause: 'agent_runtime',
        values: [
          `stack-agent: ${REVISION_A.slice(0, 12)}`,
          `stack-agent: ${B.slice(0, 12)}`,
        ],
      },
    ])
    expect(cmp.rows[1].agentRevision).toEqual({ available: true, value: B })
    expect(cmp.directlyComparable).toBe(false)
  })

  it('keeps runtime, model, task and execution differences apart', () => {
    const other = run({
      id: 'run_b',
      model: 'qwen2.5-coder-3b',
      app_version: '2.3.0',
      task: { ...run().task, version: 2, digest: 'd2' },
      agent: { ...run().agent, runtime: runtime({ commit: B }) },
    })
    const cmp = compareRuns([run(), other])
    expect(cmp.issues.map((i) => [i.cause, i.field])).toEqual([
      ['agent_runtime', 'agent_revision'],
      ['model', 'model'],
      ['task', 'task_version'],
      ['execution', 'app_version'],
    ])
  })

  it('never treats an unknown revision as the same revision', () => {
    const unmanaged = run({
      id: 'run_b',
      agent: {
        ...run().agent,
        runtime: runtime({ source: 'external', selection: 'external', commit: null }),
      },
    })
    const legacy = run({
      id: 'run_c',
      agent: { ...run().agent, runtime: undefined },
    })
    const cmp = compareRuns([run(), unmanaged, legacy])
    expect(cmp.rows[1].differences).toContain('agent_revision')
    expect(cmp.rows[2].differences).toContain('agent_revision')
    expect(cmp.rows[1].agentRevision.available).toBe(false)
    expect(cmp.rows[2].agentRevision).toEqual({
      available: false,
      reason: 'runtime not recorded',
    })
  })

  it('does not ask other agents for a Stack-Agent revision', () => {
    const claude = run({
      id: 'run_b',
      agent: {
        id: 'claude-code',
        name: 'Claude Code',
        execution_method: 'cli:claude -p',
        executable: '/bin/claude',
        version: '2.0.0',
      },
    })
    const cmp = compareRuns([run(), claude])
    expect(cmp.rows[1].differences).not.toContain('agent_revision')
    expect(cmp.rows[1].agentRevision.available).toBe(false)
  })
})

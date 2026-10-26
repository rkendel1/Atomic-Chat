import { describe, expect, it } from 'vitest'
import { stopPointOf } from '../inspect'
import type { RunInspection, WorkshopRun } from '../types'
import { run } from './fixtures'

const inspection = (r: WorkshopRun): RunInspection => ({
  run: r,
  prompt: 'Fix it',
  timeline: [],
  signals: [],
})

describe('stopPointOf', () => {
  it("prefers the agent's own terminal event", () => {
    const r: WorkshopRun = {
      ...run({ status: 'failed' }),
      agent_trace: {
        source: 'stack-agent agent-state show --json',
        available: true,
        unavailable_reason: null,
        replies: [],
        replies_unavailable_reason: null,
        events: [
          {
            index: 0,
            at: null,
            provenance: 'runtime',
            kind: 'WorkStarted',
            summary: 'work started',
          },
          {
            index: 13,
            at: null,
            provenance: 'runtime',
            kind: 'WorkEscalated',
            summary: 'work escalated: unsure',
          },
          {
            index: 14,
            at: null,
            provenance: 'observed',
            kind: 'Other',
            summary: 'later',
          },
        ],
      },
    }
    expect(stopPointOf(inspection(r))).toEqual({
      kind: 'agent_event',
      detail: 'work escalated: unsure',
      index: 13,
    })
  })

  it('falls back to what the bench observed when there is no trajectory', () => {
    const timedOut = run({
      execution: { ...run().execution!, outcome: 'timed_out', exit_code: null },
    })
    expect(stopPointOf(inspection(timedOut))).toEqual({
      kind: 'time_limit',
      detail: null,
    })
    const notStarted = run({
      execution: {
        ...run().execution!,
        outcome: 'failed_to_start',
        error: 'not installed',
      },
    })
    expect(stopPointOf(inspection(notStarted))).toEqual({
      kind: 'not_started',
      detail: 'not installed',
    })
    const exited = run({
      execution: {
        ...run().execution!,
        exit_code: 4,
        outcome: 'exited_non_zero',
      },
    })
    expect(stopPointOf(inspection(exited))).toEqual({
      kind: 'process_end',
      detail: 'exit 4',
    })
    // An unavailable trajectory is not read.
    const unavailable: WorkshopRun = {
      ...exited,
      agent_trace: {
        source: 'x',
        available: false,
        unavailable_reason: 'no db',
        events: [
          {
            index: 1,
            at: null,
            provenance: 'runtime',
            kind: 'WorkCompleted',
            summary: 'done',
          },
        ],
        replies: [],
        replies_unavailable_reason: null,
      },
    }
    expect(stopPointOf(inspection(unavailable))?.kind).toBe('process_end')
  })

  it('has no stop point before the agent ran', () => {
    expect(
      stopPointOf(inspection(run({ status: 'queued', execution: null })))
    ).toBeNull()
  })
})

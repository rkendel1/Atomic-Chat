import type { RunInspection } from './types'

/** Where a run's execution stopped, from what was recorded, or `null`. */
export type StopPoint =
  | { kind: 'not_started'; detail: string | null }
  | { kind: 'agent_event'; detail: string; index: number }
  | { kind: 'time_limit'; detail: null }
  | { kind: 'cancelled'; detail: null }
  | { kind: 'process_end'; detail: string }

const TERMINAL =
  /^Work(Completed|Escalated|Blocked|LimitReached|Failed|Cancelled)$/

export function stopPointOf(i: RunInspection): StopPoint | null {
  const run = i.run
  const ex = run.execution
  if (!ex) return null
  if (ex.outcome === 'failed_to_start')
    return { kind: 'not_started', detail: ex.error ?? null }
  // The agent's own terminal event is the most precise answer, when recorded.
  const events = run.agent_trace?.available ? run.agent_trace.events : []
  const last =
    [...events].reverse().find((e) => TERMINAL.test(e.kind)) ?? events.at(-1)
  if (last)
    return { kind: 'agent_event', detail: last.summary, index: last.index }
  if (ex.outcome === 'timed_out') return { kind: 'time_limit', detail: null }
  if (ex.outcome === 'cancelled') return { kind: 'cancelled', detail: null }
  return {
    kind: 'process_end',
    detail: `exit ${ex.exit_code ?? 'unknown'}`,
  }
}

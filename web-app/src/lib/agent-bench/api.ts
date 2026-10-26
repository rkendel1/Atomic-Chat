import { invoke } from '@tauri-apps/api/core'
import type {
  BenchOverview,
  BenchTask,
  RunConfig,
  RunRecord,
  RuntimeSelection,
  RuntimeStatus,
  Verdict,
} from './types'

/** Emitted whenever the stack-agent runtime status may have changed. */
export const RUNTIME_STATUS_EVENT = 'stack_agent_runtime:status'
/** Emitted with each line of runtime refresh or build output. */
export const RUNTIME_LOG_EVENT = 'stack_agent_runtime:log'

/** Event the Rust side emits, with the run id, whenever a run changes. */
export const RUN_UPDATED_EVENT = 'agent_bench:run_updated'

export const benchApi = {
  overview: () => invoke<BenchOverview>('bench_overview'),
  listTasks: () => invoke<BenchTask[]>('bench_list_tasks'),
  saveTask: (task: BenchTask) => invoke<BenchTask>('bench_save_task', { task }),
  deleteTask: (id: string) => invoke<void>('bench_delete_task', { id }),
  listRuns: () => invoke<RunRecord[]>('bench_list_runs'),
  getRun: (id: string) => invoke<RunRecord>('bench_get_run', { id }),
  startRun: (taskId: string, config: RunConfig) =>
    invoke<RunRecord>('bench_start_run', { taskId, config }),
  cancelRun: (id: string) => invoke<boolean>('bench_cancel_run', { id }),
  submitReview: (id: string, verdict: Verdict, note: string) =>
    invoke<RunRecord>('bench_submit_review', { id, verdict, note }),
  exportRuns: () => invoke<string>('bench_export_runs'),
}

/** The one stack-agent runtime manager (Agent Bench and the Launch page). */
export const runtimeApi = {
  status: () => invoke<RuntimeStatus>('stack_agent_runtime_status'),
  refresh: () => invoke<RuntimeStatus>('stack_agent_runtime_refresh'),
  prepareRevision: (commit: string) =>
    invoke<RuntimeStatus>('stack_agent_runtime_prepare_revision', { commit }),
  select: (selection: RuntimeSelection) =>
    invoke<RuntimeStatus>('stack_agent_runtime_select', { selection }),
  rollback: () => invoke<RuntimeStatus>('stack_agent_runtime_rollback'),
}

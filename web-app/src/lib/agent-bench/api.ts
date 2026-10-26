import { invoke } from '@tauri-apps/api/core'
import type {
  BenchOverview,
  BenchTask,
  Category,
  Confidence,
  FailureSummary,
  RegressionRequest,
  RunConfig,
  RunInspection,
  RunRecord,
  Suite,
  SuiteComparison,
  SuiteRun,
  SuiteRunView,
  Verdict,
  WorkshopRun,
} from './types'

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
  inspectRun: (id: string) =>
    invoke<RunInspection>('bench_inspect_run', { id }),
  addAssessment: (
    id: string,
    assessment: { category: Category; note: string; confidence: Confidence }
  ) => invoke<WorkshopRun>('bench_add_assessment', { id, assessment }),
  createRegression: (runId: string, request: RegressionRequest) =>
    invoke<BenchTask>('bench_create_regression', { runId, request }),
  listSuites: () => invoke<Suite[]>('bench_list_suites'),
  createSuite: (id: string, name: string, description: string) =>
    invoke<Suite>('bench_create_suite', { id, name, description }),
  addToSuite: (suiteId: string, taskId: string) =>
    invoke<Suite>('bench_add_to_suite', { suiteId, taskId }),
  removeFromSuite: (suiteId: string, taskId: string) =>
    invoke<Suite>('bench_remove_from_suite', { suiteId, taskId }),
  runSuite: (suiteId: string, config: RunConfig, label: string) =>
    invoke<SuiteRun>('bench_run_suite', { suiteId, config, label }),
  listSuiteRuns: (suiteId: string | null) =>
    invoke<SuiteRunView[]>('bench_list_suite_runs', { suiteId }),
  compareSuiteRuns: (baselineId: string, candidateId: string) =>
    invoke<SuiteComparison>('bench_compare_suite_runs', {
      baselineId,
      candidateId,
    }),
  failureSummary: (agentId: string | null) =>
    invoke<FailureSummary>('bench_failure_summary', { agentId }),
}

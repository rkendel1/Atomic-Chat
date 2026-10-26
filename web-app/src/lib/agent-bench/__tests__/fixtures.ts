import type {
  BenchTask,
  RunRecord,
  RuntimeProvenance,
  RuntimeStatus,
} from '../types'

/** A bundled-style task and a finished, verified run of it, for tests. */
export const task: BenchTask = {
  schema: 'atomic.bench.task.v1',
  id: 'ledger-fix-discount',
  version: 1,
  name: 'Fix',
  description: '',
  goal: 'Fix it',
  workdir: { kind: 'fixture', fixture: 'ledger-discount-bug' },
  success_criteria: [],
  evaluation: {
    method: 'command',
    commands: [{ name: 'tests', program: 'cargo', args: ['test'] }],
  },
  limits: {},
  builtin: true,
  created_at: '',
  updated_at: '',
}

export const REVISION_A = 'e0ca87df940d4eb866a3ea921b2f488a2c5b04b8'

/** A managed stack-agent runtime, as the runtime manager records it. */
export function runtime(
  overrides: Partial<RuntimeProvenance> = {}
): RuntimeProvenance {
  return {
    source: 'managed',
    selection: 'follow_main',
    path: '/data/stack-agent-runtime/runtimes/x86_64-linux/e0ca87df/bin/stack-agent',
    commit: REVISION_A,
    version: '0.1.0',
    platform: 'x86_64-linux',
    binary_sha256: 'f'.repeat(64),
    repository: 'https://github.com/rkendel1/stack-agent',
    toolchain: 'rustc 1.97.0',
    prepared_at: '2026-10-27T10:00:00.000Z',
    latest_known_main: REVISION_A,
    latest_checked_at: '2026-10-27T10:00:00.000Z',
    matches_latest: true,
    ...overrides,
  }
}

export function run(overrides: Partial<RunRecord> = {}): RunRecord {
  return {
    schema: 'atomic.bench.run.v1',
    id: 'run_a',
    task: {
      id: task.id,
      version: 1,
      digest: 'd1',
      name: task.name,
      snapshot: task,
    },
    agent: {
      id: 'stack-agent',
      name: 'stack-agent',
      execution_method: 'cli:stack-agent work --json',
      executable: '/bin/stack-agent',
      version: 'stack-agent 0.1.0',
      runtime: runtime(),
    },
    model: 'qwen3-8b',
    provider: 'atomic-chat-local-api',
    endpoint: 'http://127.0.0.1:1337/v1/chat/completions',
    config: { kind: 'change' },
    credential_supplied: false,
    app_version: '2.2.26',
    workdir: {
      source: 'fixture',
      path: '/data/ws/run_a',
      fixture: 'ledger-discount-bug',
    },
    status: 'succeeded',
    failure_kind: null,
    created_at: '2026-10-26T00:00:00Z',
    started_at: '2026-10-26T00:00:00Z',
    finished_at: '2026-10-26T00:01:00Z',
    duration_ms: 60000,
    execution: {
      outcome: 'exited',
      command_line: 'stack-agent work ...',
      exit_code: 0,
      duration_ms: 50000,
      stdout: { text: '', total_bytes: 0, truncated: false },
      stderr: { text: '', total_bytes: 0, truncated: false },
    },
    verification: [
      {
        name: 'tests',
        command_line: 'cargo test',
        outcome: 'passed',
        exit_code: 0,
        duration_ms: 900,
        stdout: { text: '', total_bytes: 0, truncated: false },
        stderr: { text: '', total_bytes: 0, truncated: false },
      },
    ],
    changes: {
      added: [],
      modified: ['src/pricing.rs'],
      deleted: [],
      incomplete: false,
    },
    evaluation: {
      method: 'command',
      verified: 'success',
      human: null,
      awaiting_review: false,
    },
    artifacts: [],
    errors: [],
    cancellation: null,
    interruption: null,
    history: [],
    ...overrides,
  }
}

/** A runtime status as the manager reports it after a successful refresh. */
export function runtimeStatus(
  overrides: Partial<RuntimeStatus> = {}
): RuntimeStatus {
  return {
    status: 'current',
    detail: `Revision ${REVISION_A.slice(0, 12)} is the latest main (checked 2026-10-27T10:00:00.000Z).`,
    enabled: true,
    platform: 'x86_64-linux',
    repository: 'https://github.com/rkendel1/stack-agent',
    tracked_ref: 'refs/heads/main',
    selection: { mode: 'follow_main' },
    resolved: runtime(),
    active: REVISION_A,
    previous: [],
    latest_remote: {
      commit: REVISION_A,
      checked_at: '2026-10-27T10:00:00.000Z',
    },
    last_check: { at: '2026-10-27T10:00:00.000Z', ok: true, error: null },
    last_success_refresh_at: '2026-10-27T10:00:00.000Z',
    refreshing: null,
    failure: null,
    rolled_back: [],
    cached: [],
    ...overrides,
  }
}

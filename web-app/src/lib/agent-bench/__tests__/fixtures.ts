import type { BenchTask, RunRecord } from '../types'

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

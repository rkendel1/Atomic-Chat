import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import {
  DEFAULT_FONT_SIZE,
  expectNoHorizontalOverflow,
  setFontSize,
  settle,
  withTranslations,
  XL_FONT_SIZE,
} from '@/test/layout'
import {
  run,
  runtimeStatus,
  task,
} from '@/lib/agent-bench/__tests__/fixtures'
import type { RunRecord } from '@/lib/agent-bench/types'

import { AgentBenchPage } from '../index'

vi.mock('@tanstack/react-router', () => ({
  createFileRoute: () => (config: unknown) => config,
}))
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@/lib/integration-agent-detection', () => ({
  detectIntegrationAgent: vi.fn().mockResolvedValue(true),
}))
vi.mock('@/utils/localApiServerControl', () => ({
  startLocalApiServer: vi.fn().mockResolvedValue(1337),
}))
const serviceHubStub = {
  models: () => ({ getActiveModels: vi.fn().mockResolvedValue(['qwen3-8b']) }),
  dialog: () => ({ open: vi.fn() }),
}
vi.mock('@/hooks/useServiceHub', () => ({
  useServiceHub: () => serviceHubStub,
  getServiceHub: () => serviceHubStub,
}))

const stackRun = run()
const claudeRun: RunRecord = run({
  id: 'run_b',
  agent: {
    id: 'claude-code',
    name: 'Claude Code',
    execution_method: 'cli:claude -p --output-format json',
    executable: '/usr/local/bin/claude',
    version: '2.1.310 (Claude Code)',
  },
  status: 'failed',
  failure_kind: 'verification',
  config: { permission_mode: 'acceptEdits' },
  verification: [
    { ...stackRun.verification[0], outcome: 'failed', exit_code: 101 },
  ],
  evaluation: {
    method: 'command',
    verified: 'failure',
    human: null,
    awaiting_review: false,
  },
})
const runs = [claudeRun, stackRun]

vi.mock('@/lib/agent-bench/api', () => ({
  RUN_UPDATED_EVENT: 'agent_bench:run_updated',
  RUNTIME_STATUS_EVENT: 'stack_agent_runtime:status',
  RUNTIME_LOG_EVENT: 'stack_agent_runtime:log',
  runtimeApi: {
    status: vi.fn(async () => runtimeStatus()),
    refresh: vi.fn(),
    prepareRevision: vi.fn(),
    select: vi.fn(),
    rollback: vi.fn(),
  },
  benchApi: {
    overview: vi.fn(async () => ({
      adapters: [
        {
          id: 'stack-agent',
          name: 'stack-agent',
          bin: 'stack-agent',
          execution_method: 'cli:stack-agent work --json',
          requires_model: true,
          requires_endpoint: true,
          notes:
            'Runs `stack-agent work` with --kind change. It needs pax on PATH.',
        },
        {
          id: 'claude-code',
          name: 'Claude Code',
          bin: 'claude',
          execution_method: 'cli:claude -p --output-format json',
          requires_model: false,
          requires_endpoint: false,
          notes: 'Runs `claude -p` with --permission-mode acceptEdits.',
        },
      ],
      fixtures: ['ledger', 'ledger-discount-bug'],
      suite_id: 'atomic-ledger',
      suite_version: 1,
      data_dir: '/data/agent_bench',
      app_version: '2.2.26',
    })),
    listTasks: vi.fn(async () => [task]),
    listRuns: vi.fn(async () => runs),
    getRun: vi.fn(async (id: string) => runs.find((r) => r.id === id)),
    startRun: vi.fn(),
    cancelRun: vi.fn(),
    submitReview: vi.fn(),
    exportRuns: vi.fn(),
    saveTask: vi.fn(),
    deleteTask: vi.fn(),
  },
}))

async function mount() {
  const view = render(
    withTranslations(
      <div style={{ width: 900, height: 1400 }}>
        <AgentBenchPage />
      </div>
    )
  )
  await act(async () => {
    await settle()
  })
  return view
}

describe('Agent Bench page', () => {
  beforeEach(() => setFontSize(DEFAULT_FONT_SIZE))

  it('shows the task, the run settings and the persisted history', async () => {
    const view = await mount()
    expect(await screen.findByText(task.goal)).toBeTruthy()
    expect(screen.getByText('cargo test')).toBeTruthy()
    expect(screen.getByText('Run history')).toBeTruthy()
    // The runtime the stack-agent runs will use, and its evidence.
    expect(screen.getByText('Stack-Agent runtime')).toBeTruthy()
    expect(screen.getByTestId('runtime-status').textContent).toBe('Current')
    // Both runs are listed, newest first.
    const boxes = screen.getAllByRole('checkbox', { name: 'Compare' })
    expect(boxes).toHaveLength(2)
    expectNoHorizontalOverflow(view.container.firstElementChild as HTMLElement)
  })

  it('compares two runs and shows a run in detail without overflowing', async () => {
    for (const size of [DEFAULT_FONT_SIZE, XL_FONT_SIZE]) {
      setFontSize(size)
      const view = await mount()
      const boxes = await screen.findAllByRole('checkbox', { name: 'Compare' })
      await act(async () => {
        fireEvent.click(boxes[0])
        fireEvent.click(boxes[1])
        await settle()
      })
      const table = (await screen.findByText('Comparison')).closest(
        'section'
      ) as HTMLElement
      expect(within(table).getByText(/Success/)).toBeTruthy()
      expect(within(table).getByText(/Failure/)).toBeTruthy()
      await act(async () => {
        fireEvent.click(screen.getAllByText(task.name)[0])
        await settle()
      })
      expect(await screen.findByText('Run details')).toBeTruthy()
      expect(screen.getByText('Verified outcome')).toBeTruthy()
      expectNoHorizontalOverflow(
        view.container.firstElementChild as HTMLElement
      )
      view.unmount()
    }
  })
})

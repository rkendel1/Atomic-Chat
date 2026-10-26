import { act, fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import {
  DEFAULT_FONT_SIZE,
  expectNoHorizontalOverflow,
  setFontSize,
  settle,
  withTranslations,
  XL_FONT_SIZE,
} from '@/test/layout'
import {
  REVISION_A,
  runtime,
  runtimeStatus,
} from '@/lib/agent-bench/__tests__/fixtures'
import type { RuntimeStatus } from '@/lib/agent-bench/types'

import { RuntimePanel } from '../RuntimePanel'

const B = '4850da9f22a1c6b3e2c1f0e9d8c7b6a5f4e3d2c1'

const select = vi.fn(async () => runtimeStatus())
const rollback = vi.fn(async () => runtimeStatus())

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@/lib/agent-bench/api', () => ({
  RUNTIME_STATUS_EVENT: 'stack_agent_runtime:status',
  RUNTIME_LOG_EVENT: 'stack_agent_runtime:log',
  runtimeApi: {
    status: vi.fn(),
    refresh: vi.fn(),
    prepareRevision: vi.fn(),
    select: (...a: unknown[]) => select(...(a as [])),
    rollback: (...a: unknown[]) => rollback(...(a as [])),
  },
}))

const manifest = (commit: string) => ({
  schema: 'atomic.stack_agent_runtime.manifest.v1',
  commit,
  platform: 'x86_64-linux',
  repository: 'https://github.com/rkendel1/stack-agent',
  source: 'source_build',
  version: '0.1.0',
  binary: 'bin/stack-agent',
  binary_sha256: 'f'.repeat(64),
  toolchain: 'rustc 1.97.0',
  prepared_at: '2026-10-27T10:00:00.000Z',
  validated_at: '2026-10-27T10:00:00.000Z',
  checks: [],
})

const cached = (commit: string) => ({
  commit,
  path: `/data/stack-agent-runtime/runtimes/x86_64-linux/${commit}`,
  valid: true,
  manifest: manifest(commit),
})

const STATES: Record<string, RuntimeStatus> = {
  current: runtimeStatus(),
  stale: runtimeStatus({
    status: 'stale',
    detail: `Using ${REVISION_A.slice(0, 12)} but it may be stale: the last check failed (git ls-remote failed: fatal: unable to access 'https://github.com/rkendel1/stack-agent/': Could not resolve host: github.com).`,
    last_check: {
      at: '2026-10-27T11:00:00.000Z',
      ok: false,
      error: 'Could not resolve host: github.com',
    },
  }),
  refreshing: runtimeStatus({
    status: 'refreshing',
    detail: `Building revision ${B.slice(0, 12)}.`,
    refreshing: {
      commit: B,
      stage: 'building',
      started_at: '2026-10-27T11:00:00.000Z',
      last_line:
        '   Compiling chip-core v0.1.0 (/data/stack-agent-runtime/staging/4850da9f22a1c6b3e2c1f0e9d8c7b6a5f4e3d2c1.0123/src/crates/chip-core)',
    },
  }),
  update_failed: runtimeStatus({
    status: 'update_failed',
    detail: `main is at ${B.slice(0, 12)} but preparing it failed at build: cargo build failed: error[E0425]: cannot find value \`budget\` in this scope | error: could not compile \`chip-core\` (lib) due to 1 previous error. Still using ${REVISION_A.slice(0, 12)}.`,
    latest_remote: { commit: B, checked_at: '2026-10-27T11:00:00.000Z' },
    failure: {
      commit: B,
      at: '2026-10-27T11:05:00.000Z',
      stage: 'build',
      error: 'cargo build failed',
    },
    previous: [B],
    cached: [cached(REVISION_A), cached(B)],
  }),
  pinned: runtimeStatus({
    status: 'pinned',
    detail: `Pinned to revision ${REVISION_A.slice(0, 12)}. refs/heads/main was at ${B.slice(0, 12)} when last checked; runs keep using the pinned revision.`,
    selection: { mode: 'pinned', commit: REVISION_A },
    resolved: runtime({ selection: 'pinned', matches_latest: false }),
    cached: [cached(REVISION_A), cached(B)],
  }),
  not_installed: runtimeStatus({
    status: 'not_installed',
    detail: 'No stack-agent runtime is prepared yet.',
    enabled: false,
    resolved: null,
    resolve_error:
      'stack-agent is not prepared yet. Prepare it from Agent Bench (Stack-Agent runtime) or install it from the Launch page.',
    active: null,
    latest_remote: null,
    last_check: null,
    last_success_refresh_at: null,
  }),
}

async function mount(status: RuntimeStatus, width = 420) {
  const view = render(
    withTranslations(
      <div style={{ width }}>
        <RuntimePanel initial={status} />
      </div>
    )
  )
  await act(async () => {
    await settle()
  })
  return view
}

describe('RuntimePanel', () => {
  it('shows each state with its evidence and never overflows', async () => {
    const labels: Record<string, string> = {
      current: 'Current',
      stale: 'May be stale',
      refreshing: 'Refreshing',
      update_failed: 'Update failed',
      pinned: 'Pinned',
      not_installed: 'Not prepared',
    }
    for (const size of [DEFAULT_FONT_SIZE, XL_FONT_SIZE]) {
      setFontSize(size)
      for (const [kind, status] of Object.entries(STATES)) {
        const view = await mount(status)
        expect(screen.getByTestId('runtime-status').textContent).toBe(
          labels[kind]
        )
        expect(screen.getByText(status.detail)).toBeTruthy()
        expectNoHorizontalOverflow(
          view.container.firstElementChild as HTMLElement
        )
        view.unmount()
      }
    }
  })

  it('names the revision runs use, and says when main was last seen', async () => {
    setFontSize(DEFAULT_FONT_SIZE)
    await mount(STATES.current)
    expect(screen.getAllByText(REVISION_A.slice(0, 12)).length).toBeGreaterThan(0)
    expect(screen.getByText('built by Atomic Chat')).toBeTruthy()
    expect(screen.getByRole('button', { name: /Refresh/ })).toBeTruthy()
    // Nothing to roll back to.
    expect(
      (screen.getByRole('button', { name: /Roll back/ }) as HTMLButtonElement)
        .disabled
    ).toBe(true)
  })

  it('disables refresh while refreshing and shows the build progress', async () => {
    setFontSize(DEFAULT_FONT_SIZE)
    await mount(STATES.refreshing)
    expect(
      (screen.getByRole('button', { name: /Refresh/ }) as HTMLButtonElement)
        .disabled
    ).toBe(true)
    expect(screen.getByText(/building — +Compiling chip-core/)).toBeTruthy()
  })

  it('offers rollback after a failed update and pins a cached revision on request', async () => {
    setFontSize(DEFAULT_FONT_SIZE)
    rollback.mockImplementationOnce(async () => ({
      ...STATES.update_failed,
      active: B,
      previous: [REVISION_A],
    }))
    await mount(STATES.update_failed)
    const rb = screen.getByRole('button', {
      name: /Roll back/,
    }) as HTMLButtonElement
    expect(rb.disabled).toBe(false)
    await act(async () => {
      fireEvent.click(rb)
      await settle()
    })
    expect(rollback).toHaveBeenCalled()

    const picker = screen.getByRole('combobox', { name: 'Revision for runs' })
    await act(async () => {
      fireEvent.change(picker, { target: { value: B } })
      await settle()
    })
    expect(select).toHaveBeenCalledWith({ mode: 'pinned', commit: B })
  })

  it('offers to prepare a runtime when none exists', async () => {
    setFontSize(DEFAULT_FONT_SIZE)
    await mount(STATES.not_installed)
    expect(screen.getByRole('button', { name: /Prepare/ })).toBeTruthy()
    expect(screen.getByText(/not prepared yet\. Prepare it/)).toBeTruthy()
    expect(screen.getByText('not checked yet')).toBeTruthy()
  })
})

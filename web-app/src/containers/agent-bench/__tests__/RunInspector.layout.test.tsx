import { act, fireEvent, render, screen, within } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import {
  expectNoHorizontalOverflow,
  setFontSize,
  settle,
  withTranslations,
  XL_FONT_SIZE,
  DEFAULT_FONT_SIZE,
} from '@/test/layout'
import {
  REVISION_A,
  run,
  runtimeStatus,
} from '@/lib/agent-bench/__tests__/fixtures'
import type { RunInspection, WorkshopRun } from '@/lib/agent-bench/types'

import { RunInspector } from '../RunInspector'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
const runtimeStatusCall = vi.fn()
const prepareRevision = vi.fn()
const selectRevision = vi.fn()
vi.mock('@/lib/agent-bench/api', () => ({
  benchApi: {
    cancelRun: vi.fn(),
    submitReview: vi.fn(),
    addAssessment: vi.fn(),
  },
  runtimeApi: {
    status: () => runtimeStatusCall(),
    prepareRevision: (c: string) => prepareRevision(c),
    select: (s: unknown) => selectRevision(s),
  },
}))

/** The real 3B run of ledger-fix-discount, as the workshop recorded it. */
const realFailure: WorkshopRun = {
  ...run({
    status: 'failed',
    failure_kind: 'verification',
    verification: [
      {
        ...run().verification[0],
        outcome: 'failed',
        exit_code: 101,
        started_at: '2026-10-26T18:40:23.000Z',
      } as WorkshopRun['verification'][0],
    ],
    evaluation: {
      method: 'command',
      verified: 'failure',
      human: null,
      awaiting_review: false,
    },
    changes: { added: [], modified: [], deleted: [], incomplete: false },
  }),
  baseline: {
    kind: 'fixture',
    identity:
      'fixture:ledger-discount-bug@db5e99b2470cffa2f19faa4b127172103072a84800ad43c3d0dbff3c3d1642ef',
    content_digest: 'db5e99b2',
    file_count: 10,
    git_commit: '304233de6eecd7bb1036181d6eee3bc5cb71e160',
    git_dirty: false,
    source_commit: null,
    fresh_workspace: true,
    excluded_paths: [],
    limitations: [],
  },
  agent_trace: {
    source: 'stack-agent agent-state show --json',
    available: true,
    unavailable_reason: null,
    replies: [
      {
        position: 1,
        text: '{"decision":"request_capability","capability":"project.git.diff","inputs":{}}',
        truncated: false,
      },
    ],
    replies_unavailable_reason: null,
    events: [
      {
        index: 8,
        at: '2026-10-26T18:39:50.000Z',
        provenance: 'runtime',
        kind: 'DecisionRejected',
        summary:
          'turn 1: decision rejected: invalid capability input: capability takes no inputs; the request must not carry an `inputs` member',
      },
      {
        index: 13,
        at: '2026-10-26T18:40:20.000Z',
        provenance: 'runtime',
        kind: 'WorkEscalated',
        summary:
          'work escalated: The previous request for project.git.diff was rejected due to invalid input.',
      },
    ],
  },
  analysis: {
    schema: 'atomic.bench.analysis.v1',
    rules_version: 'atomic.bench.rules.1',
    analyzed_at: '2026-10-26T18:40:24.000Z',
    status: 'failed',
    verified: 'failure',
    summary:
      'Model output or decision-format failure (rule, high confidence). The model produced replies the agent could not use as decisions.',
    causes: [
      {
        category: 'decision_format',
        source: 'rule',
        rule: 'R7-invalid-decision',
        confidence: 'high',
        primary: true,
        explanation:
          'The model produced replies the agent could not use as decisions.',
        evidence: [
          {
            class: 'agent_runtime',
            source: 'trace#8',
            detail:
              'invalid capability input: capability takes no inputs; the request must not carry an `inputs` member',
          },
        ],
        recorded_at: null,
      },
      {
        category: 'verification_failure',
        source: 'rule',
        rule: 'R13-checks-failed',
        confidence: 'high',
        primary: false,
        explanation: 'One or more verification checks failed.',
        evidence: [
          {
            class: 'verified',
            source: 'verification[Pricing tests pass]',
            detail:
              'cargo test --offline --test pricing → Failed (exit Some(101))',
          },
        ],
        recorded_at: null,
      },
    ],
    unknowns: [
      'Why the model stopped is known only from its own stated reason, which is a claim; its reasoning is not captured.',
    ],
    suggestions: [
      {
        source: 'model:example',
        category: 'context_or_discovery',
        text: 'Maybe it never read the test.',
        created_at: '',
      },
    ],
  },
}

const inspection = (
  r: WorkshopRun,
  timeline: RunInspection['timeline']
): RunInspection => ({
  run: r,
  prompt: 'The test discount_rounds_half_even in tests/pricing.rs fails.',
  timeline,
  signals: [],
})

async function mount(i: RunInspection) {
  const view = render(
    withTranslations(
      <div style={{ width: 760 }}>
        <RunInspector
          inspection={i}
          onChanged={() => {}}
          onCreateRegression={() => {}}
        />
      </div>
    )
  )
  await act(async () => {
    await settle()
  })
  return view
}

describe('RunInspector', () => {
  it('keeps observed, claimed, verified and suggested evidence apart', async () => {
    for (const size of [DEFAULT_FONT_SIZE, XL_FONT_SIZE]) {
      setFontSize(size)
      const view = await mount(
        inspection(realFailure, [
          {
            at: '2026-10-26T18:39:20.000Z',
            class: 'observed',
            origin: 'bench',
            label: 'Agent process started',
            detail: null,
            reference: 'execution.command_line',
          },
          {
            at: '2026-10-26T18:39:50.000Z',
            class: 'agent_runtime',
            origin: 'agent',
            label: 'turn 1: decision rejected',
            detail: null,
            reference: 'trace#8',
          },
          {
            at: '2026-10-26T18:39:49.000Z',
            class: 'agent_claim',
            origin: 'agent',
            label: 'turn 1: model decided: request project.git.diff',
            detail: null,
            reference: 'trace#6',
          },
          {
            at: '2026-10-26T18:40:23.000Z',
            class: 'verified',
            origin: 'verification',
            label: 'Check "Pricing tests pass": failed',
            detail: null,
            reference: 'verification[0]',
          },
        ])
      )
      expect(
        screen.getByText(
          /Model output or decision-format failure \(rule, high confidence\)/
        )
      ).toBeTruthy()
      expect(screen.getByText('primary')).toBeTruthy()
      expect(screen.getByText(/rule R7-invalid-decision/)).toBeTruthy()
      expect(
        screen.getByText(/work escalated: The previous request/)
      ).toBeTruthy()
      const suggestions = screen.getByText(
        /Suggestions \(proposed, not evidence/
      ).parentElement!
      expect(
        within(suggestions).getByText(/Maybe it never read the test/)
      ).toBeTruthy()
      expect(screen.getByText(/"inputs":\{\}/)).toBeTruthy()
      expect(screen.getByText(/fixture:ledger-discount-bug@/)).toBeTruthy()
      expect(screen.getAllByText('Agent claim').length).toBeGreaterThan(1)
      expectNoHorizontalOverflow(
        view.container.firstElementChild as HTMLElement
      )
      view.unmount()
    }
  })

  it('shows missing evidence as missing for a record from before the workshop', async () => {
    setFontSize(DEFAULT_FONT_SIZE)
    const old = run({ status: 'failed' }) as WorkshopRun
    const view = await mount(
      inspection(old, [
        {
          at: null,
          class: 'missing',
          origin: 'agent',
          label: "The agent's own trajectory was not captured for this run",
          detail: null,
          reference: 'agent_trace',
        },
      ])
    )
    expect(screen.getByText(/not analyzed yet/)).toBeTruthy()
    expect(
      screen.getByText(/this run predates baseline identities/)
    ).toBeTruthy()
    expect(
      screen.getByText(/the agent's evidence was not captured/)
    ).toBeTruthy()
    expect(
      screen.getByText(
        "The agent's own trajectory was not captured for this run"
      )
    ).toBeTruthy()
    expectNoHorizontalOverflow(view.container.firstElementChild as HTMLElement)
    view.unmount()
  })

  it('says why a trajectory is unavailable', async () => {
    setFontSize(DEFAULT_FONT_SIZE)
    const partial: WorkshopRun = {
      ...realFailure,
      agent_trace: {
        source: 'claude-code (no trajectory interface)',
        available: false,
        unavailable_reason:
          "Claude Code's `--output-format json` reports only its final result",
        events: [],
        replies: [],
        replies_unavailable_reason:
          'claude-code does not print its model replies to Agent Bench',
      },
    }
    await mount(inspection(partial, []))
    expect(screen.getByText(/does not print its model replies/)).toBeTruthy()
    // Without a trajectory the stop point comes from the process.
    expect(screen.getByText(/the agent recorded no trajectory/)).toBeTruthy()
  })

  it('shows the exact stack-agent revision a run used and reproduces it', async () => {
    for (const size of [DEFAULT_FONT_SIZE, XL_FONT_SIZE]) {
      setFontSize(size)
      const view = await mount(inspection(realFailure, []))
      expect(screen.getByText('Stack-Agent revision')).toBeTruthy()
      expect(screen.getByTitle(REVISION_A).textContent).toBe(
        REVISION_A.slice(0, 12)
      )
      expect(
        screen.getByText(/built by Atomic Chat · following main/)
      ).toBeTruthy()
      expect(screen.getByText(/sha256:ffffffffffffffff/)).toBeTruthy()
      expectNoHorizontalOverflow(
        view.container.firstElementChild as HTMLElement
      )
      view.unmount()
    }
    // Not cached any more: prepare it (it is pinned once ready).
    setFontSize(DEFAULT_FONT_SIZE)
    runtimeStatusCall.mockResolvedValueOnce(runtimeStatus({ cached: [] }))
    await mount(inspection(realFailure, []))
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Use this revision' }))
      await settle()
    })
    expect(prepareRevision).toHaveBeenCalledWith(REVISION_A)
    expect(selectRevision).not.toHaveBeenCalled()
  })

  it('says when a run has no recorded runtime, and asks only stack-agent runs', async () => {
    setFontSize(DEFAULT_FONT_SIZE)
    const legacy: WorkshopRun = {
      ...realFailure,
      agent: { ...realFailure.agent, runtime: undefined },
    }
    const view = await mount(inspection(legacy, []))
    expect(
      screen.getByText('not recorded (run predates runtime management)')
    ).toBeTruthy()
    view.unmount()
    const claude: WorkshopRun = {
      ...realFailure,
      agent: {
        id: 'claude-code',
        name: 'Claude Code',
        execution_method: 'cli:claude -p --output-format json',
        executable: '/bin/claude',
        version: '2.0.0',
      },
    }
    await mount(inspection(claude, []))
    expect(screen.queryByText('Stack-Agent revision')).toBeNull()
  })
})

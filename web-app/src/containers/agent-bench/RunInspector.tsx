import { useState, type ReactNode } from 'react'
import { toast } from 'sonner'
import { IconFolder, IconGitBranch, IconPlayerStop } from '@tabler/icons-react'
import { invoke } from '@tauri-apps/api/core'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { useTranslation } from '@/i18n/react-i18next-compat'
import { benchApi, runtimeApi } from '@/lib/agent-bench/api'
import { formatDuration } from '@/lib/agent-bench/args'
import { stopPointOf } from '@/lib/agent-bench/inspect'
import {
  CATEGORIES,
  EVIDENCE_CLASSES,
  isTerminal,
  shortCommit,
  type CapturedText,
  type Category,
  type Confidence,
  type RunInspection,
  type RuntimeProvenance,
  type Verdict,
} from '@/lib/agent-bench/types'
import { cn } from '@/lib/utils'
import { EvidenceBadge } from './EvidenceBadge'
import { StatusBadge } from './StatusBadge'

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[10rem_1fr] gap-3 py-1 text-xs">
      <span className="text-muted-foreground">{label}</span>
      <span className="min-w-0 break-words">{children}</span>
    </div>
  )
}

function Unavailable({ reason }: { reason?: string | null }) {
  const { t } = useTranslation()
  return (
    <span className="text-muted-foreground" title={reason ?? undefined}>
      {t('bench:unavailable')}
      {reason ? ` — ${reason}` : ''}
    </span>
  )
}

/**
 * The exact stack-agent runtime a run used, and a way to prepare and pin it
 * again to reproduce the run after Atomic Chat moved to a newer revision.
 */
function RuntimeRows({ runtime }: { runtime: RuntimeProvenance | null }) {
  const { t } = useTranslation()
  const [busy, setBusy] = useState(false)
  const commit = runtime?.commit ?? null
  const reproduce = async () => {
    if (!commit) return
    setBusy(true)
    try {
      const status = await runtimeApi.status()
      if (status.cached.some((c) => c.commit === commit && c.valid)) {
        await runtimeApi.select({ mode: 'pinned', commit })
        toast.success(t('bench:runtime.pinned', { commit: shortCommit(commit) }))
      } else {
        await runtimeApi.prepareRevision(commit)
        toast.info(
          t('bench:runtime.preparing', { commit: shortCommit(commit) })
        )
      }
    } catch (err) {
      toast.error(String(err))
    } finally {
      setBusy(false)
    }
  }
  return (
    <>
      <Row label={t('bench:detail.revision')}>
        {commit ? (
          <span className="flex flex-wrap items-center gap-2">
            <span className="font-mono" title={commit}>
              {shortCommit(commit)}
            </span>
            <span className="text-muted-foreground">
              {t(`bench:runtime.sourceKind.${runtime!.source}`)} ·{' '}
              {t(`bench:runtime.selectionKind.${runtime!.selection}`)}
            </span>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 px-2 text-xs"
              disabled={busy}
              onClick={reproduce}
            >
              {t('bench:runtime.reproduce')}
            </Button>
          </span>
        ) : runtime ? (
          <span className="text-muted-foreground">
            {t('bench:runtime.unmanagedRevision', {
              source: t(`bench:runtime.sourceKind.${runtime.source}`),
            })}
          </span>
        ) : (
          <span className="text-muted-foreground">
            {t('bench:runtime.notRecorded')}
          </span>
        )}
      </Row>
      {runtime?.binary_sha256 && (
        <Row label={t('bench:detail.executableDigest')}>
          <span className="font-mono">
            sha256:{runtime.binary_sha256.slice(0, 16)}…
          </span>
        </Row>
      )}
      {runtime && (
        <Row label={t('bench:detail.latestMainAtRun')}>
          {runtime.latest_known_main ? (
            <span className="font-mono">
              {shortCommit(runtime.latest_known_main)}{' '}
              <span className="font-sans text-muted-foreground">
                {runtime.matches_latest === true
                  ? t('bench:runtime.matched')
                  : runtime.matches_latest === false
                    ? t('bench:runtime.differed')
                    : t('bench:runtime.matchUnknown')}
              </span>
            </span>
          ) : (
            <Unavailable />
          )}
        </Row>
      )}
    </>
  )
}

function Output({ title, text }: { title: string; text: CapturedText }) {
  const { t } = useTranslation()
  if (!text.total_bytes) return null
  return (
    <details className="mt-1">
      <summary className="cursor-pointer text-xs text-muted-foreground">
        {title} ({text.total_bytes} B
        {text.truncated ? `, ${t('bench:detail.truncated')}` : ''})
      </summary>
      <pre className="mt-1 max-h-72 overflow-auto rounded-md bg-muted p-2 text-[11px] whitespace-pre-wrap">
        {text.text}
      </pre>
    </details>
  )
}

function Section({
  title,
  hint,
  children,
}: {
  title: string
  hint?: string
  children: ReactNode
}) {
  return (
    <section className="flex flex-col gap-1 border-t border-border/40 pt-3">
      <h3 className="text-sm font-medium text-foreground">{title}</h3>
      {hint && <p className="text-[11px] text-muted-foreground">{hint}</p>}
      {children}
    </section>
  )
}

const time = (at: string | null) =>
  at ? new Date(at).toLocaleTimeString() : '—'

/**
 * Everything recorded about one run, arranged to answer: what was asked, what
 * ran, what changed, which checks passed, where it stopped, why it failed (as
 * far as the evidence shows) and what remains unknown. Every item says what
 * kind of evidence it is.
 */
export function RunInspector({
  inspection,
  onChanged,
  onCreateRegression,
}: {
  inspection: RunInspection
  onChanged: () => void
  onCreateRegression: () => void
}) {
  const { t } = useTranslation()
  const { run, prompt, timeline } = inspection
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [causeCategory, setCauseCategory] =
    useState<Category>('goal_not_completed')
  const [causeConfidence, setCauseConfidence] = useState<Confidence>('medium')
  const [causeNote, setCauseNote] = useState('')
  const exec = run.execution
  const analysis = run.analysis
  const trace = run.agent_trace
  const origin = run.task.snapshot.origin
  const finished = isTerminal(run.status)
  const canReview =
    run.evaluation.awaiting_review || (finished && !run.evaluation.human)
  const stopAt = stopPointOf(inspection)
  const stop = stopAt
    ? t(`bench:stop.${stopAt.kind}`, {
        detail: stopAt.detail ?? '',
        index: stopAt.kind === 'agent_event' ? stopAt.index : '',
      })
    : null

  const act = async (fn: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await fn()
      onChanged()
    } catch (err) {
      toast.error(String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <StatusBadge
          status={run.status}
          awaitingReview={run.evaluation.awaiting_review}
        />
        <span className="font-medium">{run.task.name}</span>
        <span className="text-xs text-muted-foreground">
          v{run.task.version} · {run.agent.name} · {run.id}
        </span>
        <div className="ml-auto flex gap-1">
          {!finished && !run.evaluation.awaiting_review && (
            <Button
              variant="secondary"
              size="sm"
              onClick={async () => {
                if (!(await benchApi.cancelRun(run.id)))
                  toast.info(t('bench:detail.notActive'))
              }}
            >
              <IconPlayerStop size={14} />
              {t('bench:detail.cancel')}
            </Button>
          )}
          {finished && (
            <Button variant="secondary" size="sm" onClick={onCreateRegression}>
              <IconGitBranch size={14} />
              {t('bench:inspector.createRegression')}
            </Button>
          )}
        </div>
      </div>

      {run.errors.map((e, i) => (
        <p
          key={i}
          className="rounded-md bg-destructive/10 px-2 py-1 text-xs text-destructive"
        >
          {e}
        </p>
      ))}

      <Section title={t('bench:inspector.asked')}>
        <pre className="max-h-40 overflow-auto rounded-md bg-muted p-2 text-[11px] whitespace-pre-wrap">
          {prompt}
        </pre>
        <Row label={t('bench:inspector.taskVersion')}>
          {run.task.id} v{run.task.version} ·{' '}
          <code className="text-[11px]">{run.task.digest.slice(0, 12)}</code>
        </Row>
        {origin && (
          <Row label={t('bench:inspector.origin')}>
            {t('bench:inspector.originText', {
              run: origin.source_run_id,
              task: origin.source_task_id,
              version: origin.source_task_version,
            })}
            {' — '}
            {origin.failure_description}
          </Row>
        )}
      </Section>

      <Section
        title={t('bench:inspector.diagnosis')}
        hint={t('bench:inspector.diagnosisHint')}
      >
        {analysis ? (
          <>
            <p className="text-sm">{analysis.summary}</p>
            {stop && <Row label={t('bench:inspector.stoppedAt')}>{stop}</Row>}
            <ul className="flex flex-col gap-2">
              {analysis.causes.map((c, i) => (
                <li
                  key={i}
                  className={cn(
                    'rounded-md border p-2 text-xs',
                    c.primary ? 'border-foreground/30' : 'border-border/50'
                  )}
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">
                      {t(`bench:category.${c.category}`)}
                    </span>
                    {c.primary && (
                      <span className="rounded bg-foreground/10 px-1 text-[10px]">
                        {t('bench:inspector.primary')}
                      </span>
                    )}
                    <span className="text-muted-foreground">
                      {c.source === 'human'
                        ? t('bench:inspector.byReviewer')
                        : t('bench:inspector.byRule', { rule: c.rule })}
                      {' · '}
                      {t(`bench:confidence.${c.confidence}`)}
                    </span>
                  </div>
                  <p className="mt-1">{c.explanation}</p>
                  {c.evidence.length > 0 && (
                    <ul className="mt-1 flex flex-col gap-0.5">
                      {c.evidence.map((e, j) => (
                        <li key={j} className="flex items-start gap-1.5">
                          <EvidenceBadge kind={e.class} />
                          <span className="min-w-0 break-words text-muted-foreground">
                            <code className="text-[10px]">{e.source}</code>{' '}
                            {e.detail}
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}
                </li>
              ))}
            </ul>
            <div className="mt-1">
              <p className="text-xs font-medium">
                {t('bench:inspector.unknowns')}
              </p>
              {analysis.unknowns.length ? (
                <ul className="list-disc pl-5 text-xs text-muted-foreground">
                  {analysis.unknowns.map((u, i) => (
                    <li key={i}>{u}</li>
                  ))}
                </ul>
              ) : (
                <p className="text-xs text-muted-foreground">
                  {t('bench:inspector.noUnknowns')}
                </p>
              )}
            </div>
            {analysis.suggestions.length > 0 && (
              <div className="mt-1 rounded-md border border-dashed p-2 text-xs">
                <p className="font-medium">
                  {t('bench:inspector.suggestions')}
                </p>
                {analysis.suggestions.map((s, i) => (
                  <p key={i} className="text-muted-foreground">
                    {s.source}: {s.text}
                  </p>
                ))}
              </div>
            )}
            <p className="text-[10px] text-muted-foreground">
              {analysis.schema} · {analysis.rules_version}
            </p>
            {finished && (
              <div className="mt-1 flex flex-wrap items-center gap-2 rounded-md border border-dashed p-2 text-xs">
                <span>{t('bench:inspector.addCause')}</span>
                <select
                  aria-label={t('bench:inspector.category')}
                  className="h-8 rounded-md border bg-background px-1"
                  value={causeCategory}
                  onChange={(e) => setCauseCategory(e.target.value as Category)}
                >
                  {CATEGORIES.map((c) => (
                    <option key={c} value={c}>
                      {t(`bench:category.${c}`)}
                    </option>
                  ))}
                </select>
                <select
                  aria-label={t('bench:inspector.confidence')}
                  className="h-8 rounded-md border bg-background px-1"
                  value={causeConfidence}
                  onChange={(e) =>
                    setCauseConfidence(e.target.value as Confidence)
                  }
                >
                  {(['high', 'medium', 'low'] as Confidence[]).map((c) => (
                    <option key={c} value={c}>
                      {t(`bench:confidence.${c}`)}
                    </option>
                  ))}
                </select>
                <input
                  className="h-8 min-w-40 flex-1 rounded-md border bg-background px-2"
                  value={causeNote}
                  placeholder={t('bench:inspector.causeNote')}
                  onChange={(e) => setCauseNote(e.target.value)}
                />
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={busy}
                  onClick={() =>
                    act(async () => {
                      await benchApi.addAssessment(run.id, {
                        category: causeCategory,
                        confidence: causeConfidence,
                        note: causeNote,
                      })
                      setCauseNote('')
                    })
                  }
                >
                  {t('bench:inspector.record')}
                </Button>
              </div>
            )}
          </>
        ) : (
          <Unavailable reason={t('bench:inspector.notAnalyzed')} />
        )}
      </Section>

      <Section
        title={t('bench:inspector.timeline')}
        hint={t('bench:inspector.timelineHint')}
      >
        <div className="flex flex-wrap gap-1">
          {EVIDENCE_CLASSES.map((c) => (
            <EvidenceBadge key={c} kind={c} />
          ))}
        </div>
        <ol className="flex flex-col">
          {timeline.map((e, i) => (
            <li
              key={i}
              className="grid grid-cols-[5.5rem_auto_1fr] items-start gap-2 border-b border-border/30 py-1 text-xs last:border-none"
            >
              <span className="text-muted-foreground tabular-nums">
                {time(e.at)}
              </span>
              <EvidenceBadge kind={e.class} />
              <span className="min-w-0 break-words">
                {e.label}
                {e.detail && (
                  <span className="block text-[11px] text-muted-foreground">
                    {e.detail}
                  </span>
                )}
              </span>
            </li>
          ))}
        </ol>
      </Section>

      <Section title={t('bench:inspector.executed')}>
        <Row label={t('bench:detail.process')}>
          {exec ? t(`bench:exec.${exec.outcome}`) : <Unavailable />}
        </Row>
        <Row label={t('bench:detail.exitCode')}>
          {exec?.exit_code ?? <Unavailable />}
          {exec?.signal != null && ` (signal ${exec.signal})`}
        </Row>
        <Row label={t('bench:detail.duration')}>
          {run.duration_ms != null ? (
            formatDuration(run.duration_ms)
          ) : (
            <Unavailable />
          )}
          {exec &&
            ` (${t('bench:detail.agent')}: ${formatDuration(exec.duration_ms)})`}
        </Row>
        {exec?.error && (
          <Row label={t('bench:detail.error')}>
            <span className="text-destructive">{exec.error}</span>
          </Row>
        )}
        <Row label={t('bench:detail.command')}>
          {exec?.command_line ? (
            <code className="text-[11px] break-all">{exec.command_line}</code>
          ) : (
            <Unavailable />
          )}
        </Row>
        {exec && <Output title="stdout" text={exec.stdout} />}
        {exec && <Output title="stderr" text={exec.stderr} />}
        {exec?.agent_report && (
          <details className="mt-1">
            <summary className="cursor-pointer text-xs text-muted-foreground">
              {t('bench:detail.agentReport')}
            </summary>
            <pre className="mt-1 max-h-72 overflow-auto rounded-md bg-muted p-2 text-[11px]">
              {JSON.stringify(exec.agent_report, null, 2)}
            </pre>
          </details>
        )}
      </Section>

      <Section
        title={t('bench:inspector.replies')}
        hint={t('bench:inspector.repliesHint')}
      >
        {trace?.replies.length ? (
          trace.replies.map((r) => (
            <div key={r.position} className="flex items-start gap-1.5 text-xs">
              <EvidenceBadge kind="agent_claim" />
              <pre className="min-w-0 flex-1 overflow-auto rounded-md bg-muted p-1.5 text-[11px] whitespace-pre-wrap">
                {r.text}
                {r.truncated ? ' …' : ''}
              </pre>
            </div>
          ))
        ) : (
          <Unavailable
            reason={
              trace?.replies_unavailable_reason ??
              (trace ? null : t('bench:inspector.noTrace'))
            }
          />
        )}
      </Section>

      <Section title={t('bench:detail.changes')}>
        {run.changes ? (
          <>
            {(['added', 'modified', 'deleted'] as const).map((k) => (
              <Row key={k} label={t(`bench:detail.${k}`)}>
                {run.changes![k].length ? run.changes![k].join(', ') : '—'}
              </Row>
            ))}
            {run.changes.incomplete && (
              <p className="text-[11px] text-amber-600">
                {t('bench:detail.changesIncomplete')}
              </p>
            )}
          </>
        ) : (
          <Unavailable />
        )}
      </Section>

      <Section title={t('bench:detail.evaluation')}>
        <Row label={t('bench:detail.method')}>
          {t(`bench:method.${run.evaluation.method}`)}
        </Row>
        <Row label={t('bench:detail.verified')}>
          {run.evaluation.verified ? (
            t(`bench:verdict.${run.evaluation.verified}`)
          ) : (
            <Unavailable />
          )}
        </Row>
        <Row label={t('bench:detail.human')}>
          {run.evaluation.human ? (
            <>
              {t(`bench:verdict.${run.evaluation.human.verdict}`)}
              {run.evaluation.human.note && ` — ${run.evaluation.human.note}`}
            </>
          ) : (
            <Unavailable />
          )}
        </Row>
        {run.verification.map((c, i) => (
          <div key={i} className="rounded-md border border-border/50 p-2">
            <div className="flex items-center gap-2 text-xs">
              <EvidenceBadge
                kind={
                  c.outcome === 'failed_to_start' || c.outcome === 'skipped'
                    ? 'missing'
                    : 'verified'
                }
              />
              <span
                className={cn(
                  'font-medium',
                  c.outcome === 'passed'
                    ? 'text-emerald-600'
                    : 'text-destructive'
                )}
              >
                {t(`bench:check.${c.outcome}`)}
              </span>
              <span>{c.name}</span>
              <code className="ml-auto truncate text-[11px] text-muted-foreground">
                {c.command_line}
              </code>
            </div>
            <div className="text-[11px] text-muted-foreground">
              {t('bench:detail.exitCode')}: {c.exit_code ?? '—'} ·{' '}
              {formatDuration(c.duration_ms)}
              {c.error && ` · ${c.error}`}
            </div>
            <Output title="stdout" text={c.stdout} />
            <Output title="stderr" text={c.stderr} />
          </div>
        ))}
        {canReview && (
          <div className="mt-2 flex flex-col gap-2 rounded-md border border-dashed p-2">
            <p className="text-xs text-muted-foreground">
              {run.evaluation.awaiting_review
                ? run.task.snapshot.evaluation.method === 'manual' &&
                  run.task.snapshot.evaluation.instructions
                  ? run.task.snapshot.evaluation.instructions
                  : t('bench:detail.reviewPrompt')
                : t('bench:detail.reviewExtra')}
            </p>
            <Textarea
              rows={2}
              value={note}
              placeholder={t('bench:detail.notePlaceholder')}
              onChange={(e) => setNote(e.target.value)}
            />
            <div className="flex gap-2">
              {(['success', 'failure', 'inconclusive'] as Verdict[]).map(
                (v) => (
                  <Button
                    key={v}
                    size="sm"
                    variant={v === 'success' ? 'default' : 'secondary'}
                    disabled={busy}
                    onClick={() =>
                      act(async () => {
                        await benchApi.submitReview(run.id, v, note)
                        setNote('')
                      })
                    }
                  >
                    {t(`bench:verdict.${v}`)}
                  </Button>
                )
              )}
            </div>
          </div>
        )}
      </Section>

      <Section title={t('bench:inspector.reproducibility')}>
        {run.baseline ? (
          <>
            <Row label={t('bench:inspector.baseline')}>
              <code className="text-[11px] break-all">
                {run.baseline.identity}
              </code>
            </Row>
            <Row label={t('bench:inspector.workspace')}>
              {run.baseline.fresh_workspace
                ? t('bench:inspector.fresh')
                : t('bench:inspector.inPlace')}
              {run.baseline.git_commit &&
                ` · git ${run.baseline.git_commit.slice(0, 12)}`}
              {run.baseline.git_dirty && ` · ${t('bench:inspector.dirty')}`}
            </Row>
            {run.baseline.excluded_paths.length > 0 && (
              <Row label={t('bench:inspector.excluded')}>
                {run.baseline.excluded_paths.join(', ')}
              </Row>
            )}
          </>
        ) : (
          <Unavailable reason={t('bench:inspector.noBaseline')} />
        )}
        {[...(run.baseline?.limitations ?? []), ...(origin?.limitations ?? [])]
          .length > 0 && (
          <ul className="list-disc pl-5 text-[11px] text-amber-700 dark:text-amber-400">
            {[
              ...(run.baseline?.limitations ?? []),
              ...(origin?.limitations ?? []),
            ].map((l, i) => (
              <li key={i}>{l}</li>
            ))}
          </ul>
        )}
      </Section>

      <Section title={t('bench:detail.configuration')}>
        <Row label={t('bench:detail.agent')}>
          {run.agent.name} · {run.agent.execution_method}
        </Row>
        <Row label={t('bench:detail.agentVersion')}>
          {run.agent.version ?? <Unavailable />}
        </Row>
        {run.agent.id === 'stack-agent' && (
          <RuntimeRows runtime={run.agent.runtime ?? null} />
        )}
        <Row label={t('bench:detail.model')}>
          {run.model ?? <Unavailable />}
        </Row>
        <Row label={t('bench:detail.provider')}>
          {run.provider ?? <Unavailable />}
        </Row>
        <Row label={t('bench:detail.endpoint')}>
          {run.endpoint ?? <Unavailable />}
        </Row>
        <Row label={t('bench:detail.credential')}>
          {run.credential_supplied
            ? t('bench:detail.credentialSupplied')
            : t('bench:detail.credentialNone')}
        </Row>
        {Object.entries(run.config).map(([k, v]) => (
          <Row key={k} label={k}>
            {v}
          </Row>
        ))}
        <Row label={t('bench:detail.appVersion')}>{run.app_version}</Row>
        {run.suite && (
          <Row label={t('bench:inspector.suite')}>
            {run.suite.suite_id} v{run.suite.suite_version} ·{' '}
            {run.suite.suite_run_id}
          </Row>
        )}
        <Row label={t('bench:detail.workdir')}>
          <span className="break-all">{run.workdir.path}</span>
          <Button
            variant="ghost"
            size="icon-xs"
            className="ml-1 align-middle"
            aria-label={t('bench:detail.reveal')}
            onClick={() =>
              invoke('reveal_in_file_manager', {
                path: run.workdir.path,
              }).catch((e) => toast.error(String(e)))
            }
          >
            <IconFolder size={12} />
          </Button>
        </Row>
        {run.artifacts
          .filter((a) => a.kind !== 'workdir')
          .map((a, i) => (
            <Row key={i} label={a.description}>
              <code className="text-[11px] break-all">{a.reference}</code>
            </Row>
          ))}
      </Section>
    </div>
  )
}

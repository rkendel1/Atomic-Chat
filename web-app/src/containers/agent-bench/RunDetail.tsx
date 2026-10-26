import { useState, type ReactNode } from 'react'
import { toast } from 'sonner'
import { IconFolder, IconPlayerStop } from '@tabler/icons-react'
import { invoke } from '@tauri-apps/api/core'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { useTranslation } from '@/i18n/react-i18next-compat'
import { benchApi, runtimeApi } from '@/lib/agent-bench/api'
import { formatDuration } from '@/lib/agent-bench/args'
import {
  isTerminal,
  shortCommit,
  type CapturedText,
  type RunRecord,
  type RuntimeProvenance,
  type Verdict,
} from '@/lib/agent-bench/types'
import { cn } from '@/lib/utils'
import { StatusBadge } from './StatusBadge'

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[10rem_1fr] gap-3 py-1 text-xs">
      <span className="text-muted-foreground">{label}</span>
      <span className="min-w-0 break-words">{children}</span>
    </div>
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

function Unavailable() {
  const { t } = useTranslation()
  return <span className="text-muted-foreground">{t('bench:unavailable')}</span>
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

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1 border-t border-border/40 pt-3">
      <h3 className="text-sm font-medium text-foreground">{title}</h3>
      {children}
    </section>
  )
}

/** Everything recorded about one run, plus cancel and review actions. */
export function RunDetail({
  run,
  onChanged,
}: {
  run: RunRecord
  onChanged: () => void
}) {
  const { t } = useTranslation()
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const exec = run.execution
  const canReview =
    run.evaluation.awaiting_review ||
    (isTerminal(run.status) && !run.evaluation.human)

  const review = async (verdict: Verdict) => {
    setBusy(true)
    try {
      await benchApi.submitReview(run.id, verdict, note)
      setNote('')
      onChanged()
    } catch (err) {
      toast.error(String(err))
    } finally {
      setBusy(false)
    }
  }

  const cancel = async () => {
    const stopped = await benchApi.cancelRun(run.id)
    if (!stopped) toast.info(t('bench:detail.notActive'))
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
        {!isTerminal(run.status) && !run.evaluation.awaiting_review && (
          <Button
            variant="secondary"
            size="sm"
            className="ml-auto"
            onClick={cancel}
          >
            <IconPlayerStop size={14} />
            {t('bench:detail.cancel')}
          </Button>
        )}
      </div>

      {run.failure_kind && (
        <p className="text-xs text-muted-foreground">
          {t(`bench:failure.${run.failure_kind}`)}
        </p>
      )}
      {run.errors.map((e, i) => (
        <p
          key={i}
          className="rounded-md bg-destructive/10 px-2 py-1 text-xs text-destructive"
        >
          {e}
        </p>
      ))}

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
                    onClick={() => review(v)}
                  >
                    {t(`bench:verdict.${v}`)}
                  </Button>
                )
              )}
            </div>
          </div>
        )}
      </Section>

      <Section title={t('bench:detail.execution')}>
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
            <code className="text-[11px]">{exec.command_line}</code>
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

      <Section title={t('bench:detail.configuration')}>
        <Row label={t('bench:detail.agent')}>
          {run.agent.name} · {run.agent.execution_method}
        </Row>
        <Row label={t('bench:detail.agentVersion')}>
          {run.agent.version ?? <Unavailable />}
        </Row>
        <Row label={t('bench:detail.executable')}>
          {run.agent.executable ?? <Unavailable />}
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
        <Row label={t('bench:detail.workdir')}>
          {run.workdir.source === 'fixture'
            ? `${t('bench:detail.fixtureCopy', { fixture: run.workdir.fixture })} `
            : ''}
          {run.workdir.path}
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
              <code className="text-[11px]">{a.reference}</code>
            </Row>
          ))}
        <Row label={t('bench:detail.timeline')}>
          {run.history
            .map(
              (h) =>
                `${t(`bench:status.${h.status}`)} ${new Date(h.at).toLocaleTimeString()}`
            )
            .join(' → ')}
        </Row>
      </Section>
    </div>
  )
}

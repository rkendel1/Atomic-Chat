import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { IconGitBranch } from '@tabler/icons-react'
import { Button } from '@/components/ui/button'
import { useTranslation } from '@/i18n/react-i18next-compat'
import { benchApi } from '@/lib/agent-bench/api'
import type { AdapterInfo, FailureSummary } from '@/lib/agent-bench/types'

/**
 * Unsuccessful runs grouped by their primary cause, most frequent first, each
 * with its evidence: what to fix next, and the run a follow-up regression
 * task can be made from.
 */
export function FailuresPanel({
  adapters,
  refreshKey,
  onOpenRun,
  onCreateRegression,
}: {
  adapters: AdapterInfo[]
  refreshKey: number
  onOpenRun: (runId: string) => void
  onCreateRegression: (runId: string) => void
}) {
  const { t } = useTranslation()
  const [agent, setAgent] = useState('stack-agent')
  const [summary, setSummary] = useState<FailureSummary | null>(null)

  useEffect(() => {
    benchApi
      .failureSummary(agent || null)
      .then(setSummary)
      .catch((e) => toast.error(String(e)))
  }, [agent, refreshKey])

  return (
    <div className="flex flex-col gap-3 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <select
          aria-label={t('bench:run.agents')}
          className="h-9 rounded-md border bg-background px-2"
          value={agent}
          onChange={(e) => setAgent(e.target.value)}
        >
          <option value="">{t('bench:failures.allAgents')}</option>
          {adapters.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
        {summary && (
          <span className="text-xs text-muted-foreground">
            {t('bench:failures.totals', {
              considered: summary.runs_considered,
              failed: summary.not_succeeded,
              succeeded: summary.succeeded,
              excluded: summary.excluded,
            })}
          </span>
        )}
      </div>
      <p className="text-[11px] text-muted-foreground">
        {t('bench:failures.hint')}
      </p>
      {summary && summary.categories.length === 0 && (
        <p className="text-xs text-muted-foreground">
          {t('bench:failures.none')}
        </p>
      )}
      {summary?.categories.map((c) => (
        <div key={c.category} className="rounded-md border p-2">
          <div className="flex flex-wrap items-baseline gap-2">
            <span className="font-medium">
              {t(`bench:category.${c.category}`)}
            </span>
            <span className="text-xs text-muted-foreground">
              {t('bench:failures.counts', {
                primary: c.primary_count,
                contributing: c.contributing_count,
              })}
            </span>
          </div>
          <ul className="mt-1 flex flex-col gap-2">
            {c.examples.map((e) => (
              <li key={e.run_id} className="text-xs">
                <div className="flex flex-wrap items-center gap-2">
                  <button
                    className="underline"
                    onClick={() => onOpenRun(e.run_id)}
                  >
                    {e.run_id}
                  </button>
                  <span className="rounded bg-foreground/10 px-1 text-[10px]">
                    {e.primary
                      ? t('bench:inspector.primary')
                      : t('bench:failures.contributing')}
                  </span>
                  <span className="text-muted-foreground">
                    {e.task_id} v{e.task_version} · {e.agent_id} ·{' '}
                    {e.model ?? t('bench:unavailable')} ·{' '}
                    {e.source === 'human'
                      ? t('bench:inspector.byReviewer')
                      : t('bench:failures.rule')}{' '}
                    · {t(`bench:confidence.${e.confidence}`)}
                  </span>
                  <Button
                    variant="ghost"
                    size="xs"
                    className="ml-auto"
                    onClick={() => onCreateRegression(e.run_id)}
                  >
                    <IconGitBranch size={12} />
                    {t('bench:failures.followUp')}
                  </Button>
                </div>
                <p>{e.explanation}</p>
                {e.evidence.map((ev, i) => (
                  <p key={i} className="break-words text-muted-foreground">
                    · {ev}
                  </p>
                ))}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  )
}

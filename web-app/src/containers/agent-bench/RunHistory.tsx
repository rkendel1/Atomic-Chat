import { useTranslation } from '@/i18n/react-i18next-compat'
import { formatDuration } from '@/lib/agent-bench/args'
import type { RunRecord } from '@/lib/agent-bench/types'
import { cn } from '@/lib/utils'
import { StatusBadge } from './StatusBadge'

/** Every recorded run, newest first; tick runs to compare them. */
export function RunHistory({
  runs,
  openRunId,
  compareIds,
  onOpen,
  onToggleCompare,
}: {
  runs: RunRecord[]
  openRunId: string | null
  compareIds: string[]
  onOpen: (id: string) => void
  onToggleCompare: (id: string) => void
}) {
  const { t } = useTranslation()
  if (runs.length === 0)
    return (
      <p className="text-sm text-muted-foreground">
        {t('bench:history.empty')}
      </p>
    )
  return (
    <div className="flex flex-col divide-y divide-border/40 rounded-lg border">
      {runs.map((run) => (
        <div
          key={run.id}
          className={cn(
            'flex items-center gap-3 px-3 py-2 text-xs',
            openRunId === run.id && 'bg-accent/50'
          )}
        >
          <input
            type="checkbox"
            aria-label={t('bench:history.compare')}
            checked={compareIds.includes(run.id)}
            onChange={() => onToggleCompare(run.id)}
          />
          <button
            className="flex min-w-0 flex-1 items-center gap-3 text-left"
            onClick={() => onOpen(run.id)}
          >
            <StatusBadge
              status={run.status}
              awaitingReview={run.evaluation.awaiting_review}
            />
            <span className="min-w-0 flex-1 truncate">
              <span className="font-medium">{run.task.name}</span>
              <span className="text-muted-foreground">
                {' '}
                v{run.task.version}
              </span>
            </span>
            <span className="w-28 truncate">{run.agent.name}</span>
            <span className="w-36 truncate text-muted-foreground">
              {run.model ?? t('bench:unavailable')}
            </span>
            <span className="w-20 text-right text-muted-foreground">
              {run.duration_ms != null ? formatDuration(run.duration_ms) : '—'}
            </span>
            <span className="w-32 text-right text-muted-foreground">
              {new Date(run.created_at).toLocaleString()}
            </span>
          </button>
        </div>
      ))}
    </div>
  )
}

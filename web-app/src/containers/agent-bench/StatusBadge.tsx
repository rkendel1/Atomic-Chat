import { cn } from '@/lib/utils'
import { useTranslation } from '@/i18n/react-i18next-compat'
import type { RunStatus } from '@/lib/agent-bench/types'

const TONE: Record<RunStatus, string> = {
  queued: 'bg-muted text-muted-foreground',
  running: 'bg-sky-500/10 text-sky-600',
  evaluating: 'bg-violet-500/10 text-violet-600',
  succeeded: 'bg-emerald-500/10 text-emerald-600',
  failed: 'bg-destructive/10 text-destructive',
  inconclusive: 'bg-amber-500/10 text-amber-600',
  cancelled: 'bg-muted text-muted-foreground',
}

export function StatusBadge({
  status,
  awaitingReview,
}: {
  status: RunStatus
  awaitingReview?: boolean
}) {
  const { t } = useTranslation()
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center rounded-full px-1.5 py-0.5 text-[10px] font-medium',
        TONE[status]
      )}
    >
      {awaitingReview
        ? t('bench:status.awaitingReview')
        : t(`bench:status.${status}`)}
    </span>
  )
}

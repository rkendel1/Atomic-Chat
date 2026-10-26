import { useTranslation } from '@/i18n/react-i18next-compat'
import type { TimelineClass } from '@/lib/agent-bench/types'
import { cn } from '@/lib/utils'

const TONE: Record<TimelineClass, string> = {
  observed: 'bg-sky-500/10 text-sky-700 dark:text-sky-400',
  agent_runtime: 'bg-slate-500/10 text-slate-700 dark:text-slate-300',
  agent_claim: 'bg-amber-500/10 text-amber-700 dark:text-amber-400',
  verified: 'bg-emerald-500/10 text-emerald-700 dark:text-emerald-400',
  human: 'bg-violet-500/10 text-violet-700 dark:text-violet-400',
  missing:
    'border border-dashed border-muted-foreground/40 text-muted-foreground',
}

/** What kind of evidence an item is: observed, claimed, verified, human or missing. */
export function EvidenceBadge({ kind }: { kind: TimelineClass }) {
  const { t } = useTranslation()
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center rounded px-1 py-px text-[10px] font-medium whitespace-nowrap',
        TONE[kind]
      )}
    >
      {t(`bench:evidence.${kind}`)}
    </span>
  )
}

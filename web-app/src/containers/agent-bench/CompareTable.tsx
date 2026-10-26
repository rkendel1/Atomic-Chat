import type { ReactNode } from 'react'
import { useTranslation } from '@/i18n/react-i18next-compat'
import { formatDuration } from '@/lib/agent-bench/args'
import { compareRuns, type Metric } from '@/lib/agent-bench/compare'
import type { RunRecord } from '@/lib/agent-bench/types'
import { cn } from '@/lib/utils'

function Cell<T>({
  metric,
  render,
}: {
  metric: Metric<T>
  render: (v: T) => ReactNode
}) {
  const { t } = useTranslation()
  if (!metric.available)
    return (
      <span className="text-muted-foreground" title={metric.reason}>
        {t('bench:unavailable')}
      </span>
    )
  return <>{render(metric.value)}</>
}

/** Side-by-side metrics for the selected runs, with what makes them differ. */
export function CompareTable({ runs }: { runs: RunRecord[] }) {
  const { t } = useTranslation()
  const cmp = compareRuns(runs)
  const rows: { label: string; cells: ReactNode[] }[] = [
    {
      label: t('bench:compare.outcome'),
      cells: cmp.rows.map((r) => (
        <Cell
          metric={r.outcome}
          render={(o) => (
            <span
              className={cn(
                'font-medium',
                o.value === 'success' && 'text-emerald-600',
                o.value === 'failure' && 'text-destructive',
                o.value === 'inconclusive' && 'text-amber-600'
              )}
            >
              {t(`bench:verdict.${o.value}`)}
              <span className="ml-1 text-[10px] font-normal text-muted-foreground">
                ({t(`bench:compare.source.${o.source}`)})
              </span>
            </span>
          )}
        />
      )),
    },
    {
      label: t('bench:compare.duration'),
      cells: cmp.rows.map((r) => (
        <Cell metric={r.duration} render={formatDuration} />
      )),
    },
    {
      label: t('bench:compare.verification'),
      cells: cmp.rows.map((r) => (
        <Cell
          metric={r.checks}
          render={(c) =>
            t('bench:compare.checks', {
              passed: c.passed,
              failed: c.failed,
              total: c.total,
            })
          }
        />
      )),
    },
    {
      label: t('bench:compare.process'),
      cells: cmp.rows.map((r) => (
        <Cell
          metric={r.process}
          render={(p) => (
            <span title={p.error}>
              {t(`bench:exec.${p.outcome}`)}
              {p.exitCode != null && ` (${p.exitCode})`}
            </span>
          )}
        />
      )),
    },
    {
      label: t('bench:compare.changes'),
      cells: cmp.rows.map((r) => (
        <Cell
          metric={r.changes}
          render={(c) =>
            `+${c.added} ~${c.modified} −${c.deleted}${c.incomplete ? ' …' : ''}`
          }
        />
      )),
    },
    {
      label: t('bench:compare.agent'),
      cells: cmp.rows.map((r) => (
        <span>
          {r.agentName}{' '}
          <span className="text-muted-foreground">
            <Cell metric={r.agentVersion} render={(v) => v} />
          </span>
        </span>
      )),
    },
    {
      label: t('bench:compare.model'),
      cells: cmp.rows.map((r) => (
        <span>
          <Cell metric={r.model} render={(v) => v} />
          <span className="text-muted-foreground">
            {' · '}
            <Cell metric={r.provider} render={(v) => v} />
          </span>
        </span>
      )),
    },
  ]

  return (
    <div className="flex flex-col gap-2">
      {!cmp.sameTask && (
        <p className="rounded-md bg-destructive/10 px-2 py-1 text-xs text-destructive">
          {t('bench:compare.differentTasks')}
        </p>
      )}
      {cmp.issues.length > 0 && (
        <div className="rounded-md bg-amber-500/10 px-2 py-1 text-xs text-amber-700 dark:text-amber-400">
          <p className="font-medium">{t('bench:compare.notComparable')}</p>
          <ul className="list-disc pl-4">
            {cmp.issues.map((i) => (
              <li key={i.field}>
                {t(`bench:compare.field.${i.field}`)}: {i.values.join(' / ')}
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="overflow-x-auto">
        <table className="w-full text-xs">
          <thead>
            <tr className="text-left text-muted-foreground">
              <th className="py-1 pr-3 font-normal">
                {t('bench:compare.metric')}
              </th>
              {cmp.rows.map((r) => (
                <th key={r.runId} className="py-1 pr-3 font-normal">
                  <div className="font-medium text-foreground">
                    {r.agentName}
                  </div>
                  <div className="truncate">{r.runId}</div>
                  {r.differences.length > 0 && (
                    <div className="text-amber-600">
                      {t('bench:compare.differs', {
                        fields: r.differences
                          .map((f) => t(`bench:compare.field.${f}`))
                          .join(', '),
                      })}
                    </div>
                  )}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.label} className="border-t border-border/40">
                <td className="py-1.5 pr-3 text-muted-foreground">
                  {row.label}
                </td>
                {row.cells.map((c, i) => (
                  <td key={i} className="py-1.5 pr-3">
                    {c}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}

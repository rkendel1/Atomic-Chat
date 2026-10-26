import { useCallback, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { IconPlayerPlay, IconPlus, IconTrash } from '@tabler/icons-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useTranslation } from '@/i18n/react-i18next-compat'
import { benchApi } from '@/lib/agent-bench/api'
import { formatDuration } from '@/lib/agent-bench/args'
import { formatRate } from '@/lib/agent-bench/suites'
import type {
  AdapterInfo,
  BenchTask,
  RunConfig,
  Suite,
  SuiteComparison,
  SuiteRunSummary,
  SuiteRunView,
} from '@/lib/agent-bench/types'
import { cn } from '@/lib/utils'

function Rate({ s }: { s: SuiteRunSummary }) {
  const { t } = useTranslation()
  return (
    <span>
      {t('bench:suites.rate', {
        rate: formatRate(s.verified_success_rate),
        successes: s.verified_successes,
        denominator: s.denominator,
      })}
      {s.exclusions.length > 0 && (
        <span className="text-muted-foreground">
          {' '}
          {t('bench:suites.excluded', {
            count: s.exclusions.length,
            total: s.total,
          })}
        </span>
      )}
    </span>
  )
}

const CHANGE_TONE: Record<string, string> = {
  improvement: 'text-emerald-600',
  regression: 'text-destructive',
  still_fails: 'text-muted-foreground',
  still_succeeds: 'text-muted-foreground',
  not_comparable: 'text-amber-600',
}

export function SuiteComparisonView({ cmp }: { cmp: SuiteComparison }) {
  const { t } = useTranslation()
  return (
    <div className="flex flex-col gap-2 text-xs">
      {cmp.warnings.length > 0 && (
        <div className="rounded-md bg-amber-500/10 px-2 py-1 text-amber-700 dark:text-amber-400">
          <p className="font-medium">{t('bench:suites.notComparable')}</p>
          <ul className="list-disc pl-4">
            {cmp.warnings.map((w) => (
              <li key={w.field}>
                {w.field}: {w.baseline} → {w.candidate}
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="grid grid-cols-2 gap-2">
        {(['baseline', 'candidate'] as const).map((side) => (
          <div key={side} className="rounded-md border p-2">
            <p className="font-medium">{t(`bench:suites.${side}`)}</p>
            <Rate s={cmp[side]} />
            {cmp[side].exclusions.length > 0 && (
              <ul className="mt-1 list-disc pl-4 text-muted-foreground">
                {cmp[side].exclusions.map((e) => (
                  <li key={e.task_id}>
                    {e.task_id}: {e.reason}
                  </li>
                ))}
              </ul>
            )}
            <p className="mt-1 text-muted-foreground">
              {t('bench:suites.duration', {
                total: formatDuration(cmp[side].total_duration_ms),
                recorded: cmp[side].durations_recorded,
                count: cmp[side].total,
              })}
            </p>
          </div>
        ))}
      </div>
      <p>
        {t('bench:suites.changes', {
          improvements: cmp.improvements,
          regressions: cmp.regressions,
          notComparable: cmp.not_comparable,
        })}
      </p>
      <table className="w-full">
        <thead>
          <tr className="text-left text-muted-foreground">
            <th className="py-1 pr-2 font-normal">{t('bench:suites.task')}</th>
            <th className="py-1 pr-2 font-normal">
              {t('bench:suites.baseline')}
            </th>
            <th className="py-1 pr-2 font-normal">
              {t('bench:suites.candidate')}
            </th>
            <th className="py-1 pr-2 font-normal">
              {t('bench:suites.change')}
            </th>
          </tr>
        </thead>
        <tbody>
          {cmp.tasks.map((x) => (
            <tr key={x.task_id} className="border-t border-border/40">
              <td className="py-1 pr-2">{x.task_id}</td>
              <td className="py-1 pr-2">
                {x.baseline ? t(`bench:result.${x.baseline}`) : '—'}
                {x.baseline_duration_ms != null &&
                  ` · ${formatDuration(x.baseline_duration_ms)}`}
              </td>
              <td className="py-1 pr-2">
                {x.candidate ? t(`bench:result.${x.candidate}`) : '—'}
                {x.candidate_duration_ms != null &&
                  ` · ${formatDuration(x.candidate_duration_ms)}`}
              </td>
              <td
                className={cn('py-1 pr-2 font-medium', CHANGE_TONE[x.change])}
              >
                {t(`bench:change.${x.change}`)}
                {x.reason && (
                  <span className="block font-normal text-muted-foreground">
                    {x.reason}
                  </span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/** Regression suites: pinned tasks, runs against one configuration, comparisons. */
export function SuitesPanel({
  adapters,
  currentTask,
  refreshKey,
  buildConfig,
  onOpenRun,
}: {
  adapters: AdapterInfo[]
  currentTask: BenchTask | null
  refreshKey: number
  buildConfig: (agentId: string) => Promise<RunConfig>
  onOpenRun: (runId: string) => void
}) {
  const { t } = useTranslation()
  const [suites, setSuites] = useState<Suite[]>([])
  const [suiteId, setSuiteId] = useState('')
  const [views, setViews] = useState<SuiteRunView[]>([])
  const [newId, setNewId] = useState('')
  const [newName, setNewName] = useState('')
  const [agentId, setAgentId] = useState('stack-agent')
  const [label, setLabel] = useState('')
  const [baselineId, setBaselineId] = useState('')
  const [candidateId, setCandidateId] = useState('')
  const [cmp, setCmp] = useState<SuiteComparison | null>(null)
  const [busy, setBusy] = useState(false)
  const suite = suites.find((s) => s.id === suiteId) ?? null

  const load = useCallback(async () => {
    const list = await benchApi.listSuites()
    setSuites(list)
    setSuiteId((cur) =>
      cur && list.some((s) => s.id === cur) ? cur : (list[0]?.id ?? '')
    )
  }, [])

  useEffect(() => {
    load().catch((e) => toast.error(String(e)))
  }, [load])

  useEffect(() => {
    if (!suiteId) return setViews([])
    benchApi
      .listSuiteRuns(suiteId)
      .then(setViews)
      .catch((e) => toast.error(String(e)))
  }, [suiteId, refreshKey])

  useEffect(() => {
    if (!baselineId || !candidateId || baselineId === candidateId)
      return setCmp(null)
    benchApi
      .compareSuiteRuns(baselineId, candidateId)
      .then(setCmp)
      .catch((e) => toast.error(String(e)))
  }, [baselineId, candidateId, refreshKey])

  const act = async (fn: () => Promise<void>) => {
    setBusy(true)
    try {
      await fn()
    } catch (e) {
      toast.error(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex flex-col gap-4 text-sm">
      <div className="flex flex-wrap items-end gap-2">
        <label className="flex flex-col gap-1">
          <span className="text-xs text-muted-foreground">
            {t('bench:suites.suite')}
          </span>
          <select
            aria-label={t('bench:suites.suite')}
            className="h-9 min-w-56 rounded-md border bg-background px-2"
            value={suiteId}
            onChange={(e) => setSuiteId(e.target.value)}
          >
            {suites.length === 0 && (
              <option value="">{t('bench:suites.none')}</option>
            )}
            {suites.map((s) => (
              <option key={s.id} value={s.id}>
                {s.name} (v{s.version}, {s.entries.length})
              </option>
            ))}
          </select>
        </label>
        <Input
          className="w-40"
          value={newId}
          placeholder={t('bench:suites.newId')}
          onChange={(e) => setNewId(e.target.value)}
        />
        <Input
          className="w-48"
          value={newName}
          placeholder={t('bench:suites.newName')}
          onChange={(e) => setNewName(e.target.value)}
        />
        <Button
          variant="secondary"
          size="sm"
          disabled={busy || !newId.trim() || !newName.trim()}
          onClick={() =>
            act(async () => {
              const s = await benchApi.createSuite(
                newId.trim(),
                newName.trim(),
                ''
              )
              setNewId('')
              setNewName('')
              await load()
              setSuiteId(s.id)
            })
          }
        >
          <IconPlus size={14} />
          {t('bench:suites.create')}
        </Button>
      </div>

      {suite && (
        <>
          <div className="flex flex-col gap-1">
            <div className="flex items-center gap-2">
              <span className="font-medium">
                {t('bench:suites.entries', { version: suite.version })}
              </span>
              {currentTask && (
                <Button
                  variant="ghost"
                  size="sm"
                  className="ml-auto"
                  disabled={busy}
                  onClick={() =>
                    act(async () => {
                      await benchApi.addToSuite(suite.id, currentTask.id)
                      await load()
                    })
                  }
                >
                  <IconPlus size={14} />
                  {t('bench:suites.add', {
                    task: currentTask.id,
                    version: currentTask.version,
                  })}
                </Button>
              )}
            </div>
            {suite.entries.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                {t('bench:suites.empty')}
              </p>
            ) : (
              <table className="w-full text-xs">
                <thead>
                  <tr className="text-left text-muted-foreground">
                    <th className="py-1 pr-2 font-normal">
                      {t('bench:suites.task')}
                    </th>
                    <th className="py-1 pr-2 font-normal">
                      {t('bench:suites.expected')}
                    </th>
                    <th className="py-1 pr-2 font-normal">
                      {t('bench:suites.baselineId')}
                    </th>
                    <th className="py-1 pr-2 font-normal">
                      {t('bench:suites.verification')}
                    </th>
                    <th className="py-1 pr-2 font-normal">
                      {t('bench:suites.source')}
                    </th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {suite.entries.map((e) => (
                    <tr
                      key={e.task_id}
                      className="border-t border-border/40 align-top"
                    >
                      <td className="py-1 pr-2">
                        {e.task_name}
                        <span className="block text-muted-foreground">
                          {e.task_id} v{e.task_version}
                        </span>
                      </td>
                      <td className="py-1 pr-2">
                        {t(`bench:verdict.${e.expected}`)}
                      </td>
                      <td className="py-1 pr-2 break-all">{e.baseline}</td>
                      <td className="py-1 pr-2">
                        {e.verification.map((v, i) => (
                          <code key={i} className="block text-[10px]">
                            {v}
                          </code>
                        ))}
                      </td>
                      <td className="py-1 pr-2">
                        {e.source_run_id ? (
                          <button
                            className="text-left underline"
                            onClick={() => onOpenRun(e.source_run_id!)}
                          >
                            {e.source_run_id}
                          </button>
                        ) : (
                          '—'
                        )}
                      </td>
                      <td>
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          aria-label={t('bench:suites.remove')}
                          onClick={() =>
                            act(async () => {
                              await benchApi.removeFromSuite(
                                suite.id,
                                e.task_id
                              )
                              await load()
                            })
                          }
                        >
                          <IconTrash size={12} />
                        </Button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="flex flex-wrap items-end gap-2">
            <label className="flex flex-col gap-1">
              <span className="text-xs text-muted-foreground">
                {t('bench:run.agents')}
              </span>
              <select
                className="h-9 rounded-md border bg-background px-2"
                value={agentId}
                onChange={(e) => setAgentId(e.target.value)}
              >
                {adapters.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name}
                  </option>
                ))}
              </select>
            </label>
            <Input
              className="w-56"
              value={label}
              placeholder={t('bench:suites.label')}
              onChange={(e) => setLabel(e.target.value)}
            />
            <Button
              size="sm"
              disabled={busy || suite.entries.length === 0}
              onClick={() =>
                act(async () => {
                  const config = await buildConfig(agentId)
                  await benchApi.runSuite(suite.id, config, label)
                  setLabel('')
                  setViews(await benchApi.listSuiteRuns(suite.id))
                })
              }
            >
              <IconPlayerPlay size={14} />
              {t('bench:suites.run')}
            </Button>
            <span className="text-[11px] text-muted-foreground">
              {t('bench:suites.runHint')}
            </span>
          </div>

          <div className="flex flex-col gap-1">
            <span className="font-medium">{t('bench:suites.runs')}</span>
            {views.length === 0 ? (
              <p className="text-xs text-muted-foreground">
                {t('bench:suites.noRuns')}
              </p>
            ) : (
              <table className="w-full text-xs">
                <thead>
                  <tr className="text-left text-muted-foreground">
                    <th className="py-1 pr-2 font-normal">
                      {t('bench:suites.baseline')}
                    </th>
                    <th className="py-1 pr-2 font-normal">
                      {t('bench:suites.candidate')}
                    </th>
                    <th className="py-1 pr-2 font-normal">
                      {t('bench:suites.run')}
                    </th>
                    <th className="py-1 pr-2 font-normal">
                      {t('bench:suites.result')}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {views.map(({ suite_run: sr, summary }) => (
                    <tr
                      key={sr.id}
                      className="border-t border-border/40 align-top"
                    >
                      <td className="py-1 pr-2">
                        <input
                          type="radio"
                          name="suite-baseline"
                          aria-label={t('bench:suites.baseline')}
                          checked={baselineId === sr.id}
                          onChange={() => setBaselineId(sr.id)}
                        />
                      </td>
                      <td className="py-1 pr-2">
                        <input
                          type="radio"
                          name="suite-candidate"
                          aria-label={t('bench:suites.candidate')}
                          checked={candidateId === sr.id}
                          onChange={() => setCandidateId(sr.id)}
                        />
                      </td>
                      <td className="py-1 pr-2">
                        <span className="font-medium">{sr.label || sr.id}</span>
                        <span className="block text-muted-foreground">
                          {sr.agent_id} · {sr.model ?? t('bench:unavailable')} ·
                          v{sr.suite_version} ·{' '}
                          {new Date(sr.created_at).toLocaleString()}
                        </span>
                      </td>
                      <td className="py-1 pr-2">
                        <Rate s={summary} />
                        <span className="block text-muted-foreground">
                          {Object.entries(summary.counts)
                            .map(([k, n]) => `${t(`bench:result.${k}`)}: ${n}`)
                            .join(' · ')}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          {cmp && (
            <div className="flex flex-col gap-1">
              <span className="font-medium">
                {t('bench:suites.comparison')}
              </span>
              <SuiteComparisonView cmp={cmp} />
            </div>
          )}
        </>
      )}
    </div>
  )
}

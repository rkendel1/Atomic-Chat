import { createFileRoute } from '@tanstack/react-router'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { toast } from 'sonner'
import {
  IconCopy,
  IconDownload,
  IconEdit,
  IconLoader2,
  IconPlayerPlay,
  IconPlus,
  IconTrash,
} from '@tabler/icons-react'
import { route } from '@/constants/routes'
import { INTEGRATION_AGENTS } from '@/constants/integrations'
import HeaderPage from '@/containers/HeaderPage'
import { Card } from '@/containers/Card'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useTranslation } from '@/i18n/react-i18next-compat'
import { useLocalApiServer } from '@/hooks/useLocalApiServer'
import { useAppState } from '@/hooks/useAppState'
import { useServiceHub } from '@/hooks/useServiceHub'
import { useLaunchStore } from '@/stores/launch-store'
import { useLaunchSettings } from '@/stores/launch-settings-store'
import { detectIntegrationAgent } from '@/lib/integration-agent-detection'
import { agentApiUrl } from '@/lib/integration-agent-setup'
import { startLocalApiServer } from '@/utils/localApiServerControl'
import { createSafeUnlisten } from '@/lib/tauriEvent'
import { benchApi, RUN_UPDATED_EVENT } from '@/lib/agent-bench/api'
import type {
  BenchOverview,
  BenchTask,
  RunRecord,
} from '@/lib/agent-bench/types'
import { TaskEditorDialog } from '@/containers/agent-bench/TaskEditorDialog'
import { RunHistory } from '@/containers/agent-bench/RunHistory'
import { RunDetail } from '@/containers/agent-bench/RunDetail'
import { CompareTable } from '@/containers/agent-bench/CompareTable'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export const Route = createFileRoute(route.bench.index as any)({
  component: AgentBenchPage,
})

function Section({
  title,
  description,
  actions,
  children,
}: {
  title: string
  description?: string
  actions?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <section className="flex flex-col gap-3">
      <div className="flex items-end gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="font-studio text-lg font-medium text-foreground">
            {title}
          </h1>
          {description && (
            <p className="text-sm text-muted-foreground">{description}</p>
          )}
        </div>
        {actions}
      </div>
      {children}
    </section>
  )
}

export function AgentBenchPage() {
  const { t } = useTranslation()
  const serviceHub = useServiceHub()
  const {
    serverHost,
    serverPort,
    apiPrefix,
    apiKey,
    defaultModelLocalApiServer,
  } = useLocalApiServer()
  const { serverStatus, setServerStatus } = useAppState()
  const installed = useLaunchStore((s) => s.installed)
  const binPath = useLaunchStore((s) => s.binPath)
  const customPaths = useLaunchSettings((s) => s.customPaths)

  const [overview, setOverview] = useState<BenchOverview | null>(null)
  const [tasks, setTasks] = useState<BenchTask[]>([])
  const [runs, setRuns] = useState<RunRecord[]>([])
  const [taskId, setTaskId] = useState<string>('')
  const [agentIds, setAgentIds] = useState<string[]>(['stack-agent'])
  const [model, setModel] = useState('')
  const [runningModels, setRunningModels] = useState<string[]>([])
  const [claudeUsesLocal, setClaudeUsesLocal] = useState(true)
  const [starting, setStarting] = useState(false)
  const [openRun, setOpenRun] = useState<RunRecord | null>(null)
  const openRunId = openRun?.id ?? null
  const [compareIds, setCompareIds] = useState<string[]>([])
  const [compareRuns, setCompareRuns] = useState<RunRecord[]>([])
  const [editor, setEditor] = useState<{ initial: BenchTask | null } | null>(
    null
  )
  const [loadError, setLoadError] = useState<string | null>(null)

  const task = tasks.find((x) => x.id === taskId) ?? null

  const loadRuns = useCallback(async () => {
    try {
      setRuns(await benchApi.listRuns())
    } catch (err) {
      setLoadError(String(err))
    }
  }, [])

  const loadTasks = useCallback(async () => {
    const list = await benchApi.listTasks()
    setTasks(list)
    setTaskId((current) =>
      current && list.some((x) => x.id === current)
        ? current
        : (list[0]?.id ?? '')
    )
  }, [])

  useEffect(() => {
    Promise.all([
      benchApi.overview().then(setOverview),
      loadTasks(),
      loadRuns(),
    ])
      .then(() => setLoadError(null))
      .catch((err) => setLoadError(String(err)))
  }, [loadTasks, loadRuns])

  // Detect the agents the bench can drive, the same way the Launch page does.
  useEffect(() => {
    for (const adapter of overview?.adapters ?? []) {
      const agent = INTEGRATION_AGENTS.find((a) => a.id === adapter.id)
      if (agent) detectIntegrationAgent(agent, customPaths[agent.id])
    }
  }, [overview, customPaths])

  useEffect(() => {
    serviceHub
      .models()
      .getActiveModels()
      .then((active) => setRunningModels(active || []))
      .catch(() => setRunningModels([]))
  }, [serviceHub, serverStatus])

  // The model the Local API Server routes, as on the Launch page.
  const activeModel =
    runningModels[0] ?? defaultModelLocalApiServer?.model ?? ''
  useEffect(() => {
    setModel((m) => m || activeModel)
  }, [activeModel])

  // Refresh on every run change the Rust side reports.
  const openRef = useRef<string | null>(null)
  openRef.current = openRunId
  const compareRef = useRef<string[]>([])
  compareRef.current = compareIds
  useEffect(() => {
    let detach: (() => void) | undefined
    let disposed = false
    listen<string>(RUN_UPDATED_EVENT, (event) => {
      loadRuns()
      if (event.payload === openRef.current)
        benchApi
          .getRun(event.payload)
          .then(setOpenRun)
          .catch(() => {})
      if (compareRef.current.includes(event.payload))
        benchApi
          .getRun(event.payload)
          .then((r) =>
            setCompareRuns((all) => all.map((x) => (x.id === r.id ? r : x)))
          )
          .catch(() => {})
    }).then((fn) => {
      detach = createSafeUnlisten(fn)
      if (disposed) detach()
    })
    return () => {
      disposed = true
      detach?.()
    }
  }, [loadRuns])

  const open = async (id: string) => {
    try {
      setOpenRun(await benchApi.getRun(id))
    } catch (err) {
      toast.error(String(err))
    }
  }

  const toggleCompare = async (id: string) => {
    if (compareIds.includes(id)) {
      setCompareIds((ids) => ids.filter((x) => x !== id))
      setCompareRuns((all) => all.filter((r) => r.id !== id))
      return
    }
    const run = await benchApi.getRun(id)
    setCompareIds((ids) => [...ids, id])
    setCompareRuns((all) => [...all, run])
  }

  const ensureServerRunning = async () => {
    if (serverStatus === 'running') return
    try {
      await startLocalApiServer()
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      if (!msg.includes('already running')) throw err
    }
    setServerStatus('running')
  }

  const run = async () => {
    if (!task) return
    setStarting(true)
    try {
      const adapters =
        overview?.adapters.filter((a) => agentIds.includes(a.id)) ?? []
      const needsServer = adapters.some(
        (a) =>
          a.requires_endpoint || (a.id === 'claude-code' && claudeUsesLocal)
      )
      if (needsServer) await ensureServerRunning()
      let first: RunRecord | null = null
      // One after another: the Rust side queues them and runs one at a time.
      for (const adapter of adapters) {
        const agent = INTEGRATION_AGENTS.find((a) => a.id === adapter.id)
        const useLocal = adapter.id !== 'claude-code' || claudeUsesLocal
        const apiUrl =
          agent && useLocal
            ? agentApiUrl(agent, { serverHost, serverPort, apiPrefix, apiKey })
            : null
        const record = await benchApi.startRun(task.id, {
          agent_id: adapter.id,
          model: model.trim() || null,
          api_url: apiUrl,
          api_key: useLocal ? apiKey || null : null,
          executable: customPaths[adapter.id] || binPath[adapter.id] || null,
        })
        first ??= record
      }
      if (first) setOpenRun(first)
      await loadRuns()
    } catch (err) {
      toast.error(t('bench:run.failedToStart'), { description: String(err) })
    } finally {
      setStarting(false)
    }
  }

  const exportRuns = async () => {
    try {
      const path = await benchApi.exportRuns()
      toast.success(t('bench:export.done'), { description: path })
      invoke('reveal_in_file_manager', { path }).catch(() => {})
    } catch (err) {
      toast.error(t('bench:export.failed'), { description: String(err) })
    }
  }

  const deleteTask = async () => {
    if (!task || task.builtin) return
    try {
      await benchApi.deleteTask(task.id)
      setTaskId('')
      await loadTasks()
    } catch (err) {
      toast.error(String(err))
    }
  }

  const evaluation = task?.evaluation
  const adapters = overview?.adapters ?? []
  const needsModel = adapters.some(
    (a) => agentIds.includes(a.id) && a.requires_model
  )
  const canRun =
    !!task &&
    agentIds.length > 0 &&
    !starting &&
    (!needsModel || !!model.trim())
  const modelOptions = useMemo(
    () => [...new Set([activeModel, ...runningModels].filter(Boolean))],
    [activeModel, runningModels]
  )

  return (
    <div className="flex h-svh w-full flex-col">
      <HeaderPage>
        <div className="flex w-full items-center gap-2">
          <span className="font-studio text-base font-medium">
            {t('bench:title')}
          </span>
          <Button
            variant="secondary"
            size="sm"
            className="ml-auto"
            onClick={exportRuns}
          >
            <IconDownload size={14} />
            {t('bench:export.button')}
          </Button>
        </div>
      </HeaderPage>
      <div className="h-[calc(100%-60px)] overflow-y-auto p-4 pt-0">
        <div className="mx-auto flex max-w-4xl flex-col gap-6">
          {loadError && (
            <p className="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
              {loadError}
            </p>
          )}

          <Section
            title={t('bench:task.title')}
            description={t('bench:task.description')}
            actions={
              <div className="flex gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setEditor({ initial: null })}
                >
                  <IconPlus size={14} />
                  {t('bench:task.new')}
                </Button>
                {task && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() =>
                      setEditor({ initial: { ...task, builtin: true } })
                    }
                  >
                    <IconCopy size={14} />
                    {t('bench:task.duplicate')}
                  </Button>
                )}
                {task && !task.builtin && (
                  <>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => setEditor({ initial: task })}
                    >
                      <IconEdit size={14} />
                      {t('bench:task.edit')}
                    </Button>
                    <Button variant="ghost" size="sm" onClick={deleteTask}>
                      <IconTrash size={14} />
                    </Button>
                  </>
                )}
              </div>
            }
          >
            <Card className="rounded-lg bg-card">
              <select
                aria-label={t('bench:task.select')}
                className="h-9 w-full rounded-md border bg-background px-2 text-sm"
                value={taskId}
                onChange={(e) => setTaskId(e.target.value)}
              >
                {tasks.map((x) => (
                  <option key={x.id} value={x.id}>
                    {x.builtin ? `★ ${x.name}` : x.name} (v{x.version})
                  </option>
                ))}
              </select>
              {task && (
                <div className="mt-3 flex flex-col gap-2 text-sm">
                  {task.description && (
                    <p className="text-muted-foreground">{task.description}</p>
                  )}
                  <p className="whitespace-pre-wrap">{task.goal}</p>
                  {task.success_criteria.length > 0 && (
                    <ul className="list-disc pl-5 text-xs text-muted-foreground">
                      {task.success_criteria.map((c, i) => (
                        <li key={i}>{c}</li>
                      ))}
                    </ul>
                  )}
                  <div className="text-xs">
                    <span className="text-muted-foreground">
                      {t('bench:task.project')}:{' '}
                    </span>
                    {task.workdir.kind === 'fixture'
                      ? t('bench:task.fixture', {
                          fixture: task.workdir.fixture,
                        })
                      : task.workdir.path}
                  </div>
                  <div className="text-xs">
                    <span className="text-muted-foreground">
                      {t('bench:task.evaluation')}:{' '}
                    </span>
                    {evaluation?.method === 'manual'
                      ? t('bench:method.manual')
                      : t('bench:method.command')}
                  </div>
                  {evaluation?.method === 'command' && (
                    <ul className="flex flex-col gap-0.5 text-xs">
                      {evaluation.commands.map((c, i) => (
                        <li key={i}>
                          {c.name}:{' '}
                          <code className="rounded bg-muted px-1 text-[11px]">
                            {[c.program, ...c.args].join(' ')}
                          </code>
                        </li>
                      ))}
                      {evaluation.held_out_checks && (
                        <li className="text-muted-foreground">
                          {t('bench:task.heldOut', {
                            set: evaluation.held_out_checks,
                          })}
                        </li>
                      )}
                    </ul>
                  )}
                  <div className="text-xs text-muted-foreground">
                    {t('bench:task.limits', {
                      timeout: task.limits.timeout_secs ?? 900,
                      turns:
                        task.limits.max_turns ?? t('bench:editor.agentDefault'),
                    })}
                  </div>
                </div>
              )}
            </Card>
          </Section>

          <Section
            title={t('bench:run.title')}
            description={t('bench:run.description')}
          >
            <Card className="rounded-lg bg-card">
              <div className="flex flex-col gap-3 text-sm">
                <div className="flex flex-col gap-1">
                  <span className="text-xs font-medium text-muted-foreground">
                    {t('bench:run.agents')}
                  </span>
                  {adapters.map((a) => (
                    <label key={a.id} className="flex items-start gap-2">
                      <input
                        type="checkbox"
                        className="mt-1"
                        checked={agentIds.includes(a.id)}
                        onChange={() =>
                          setAgentIds((ids) =>
                            ids.includes(a.id)
                              ? ids.filter((x) => x !== a.id)
                              : [...ids, a.id]
                          )
                        }
                      />
                      <span className="min-w-0">
                        <span className="font-medium">{a.name}</span>{' '}
                        <span className="text-xs text-muted-foreground">
                          {installed[a.id] === undefined
                            ? ''
                            : installed[a.id]
                              ? t('bench:run.installed')
                              : t('bench:run.notInstalled')}
                        </span>
                        <span className="block text-xs text-muted-foreground">
                          {a.notes}
                        </span>
                        {a.id === 'claude-code' && agentIds.includes(a.id) && (
                          <label className="mt-1 flex items-center gap-2 text-xs">
                            <input
                              type="checkbox"
                              checked={claudeUsesLocal}
                              onChange={(e) =>
                                setClaudeUsesLocal(e.target.checked)
                              }
                            />
                            {t('bench:run.claudeUsesLocal')}
                          </label>
                        )}
                      </span>
                    </label>
                  ))}
                </div>
                <label className="flex flex-col gap-1">
                  <span className="text-xs font-medium text-muted-foreground">
                    {t('bench:run.model')}
                  </span>
                  <Input
                    list="bench-models"
                    value={model}
                    placeholder={t('bench:run.modelPlaceholder')}
                    onChange={(e) => setModel(e.target.value)}
                  />
                  <datalist id="bench-models">
                    {modelOptions.map((m) => (
                      <option key={m} value={m} />
                    ))}
                  </datalist>
                  <span className="text-[11px] text-muted-foreground">
                    {serverStatus === 'running'
                      ? t('bench:run.serverRunning', {
                          url: `http://${serverHost}:${serverPort}${apiPrefix}`,
                        })
                      : t('bench:run.serverStopped')}
                  </span>
                </label>
                <div className="flex items-center gap-2">
                  <Button onClick={run} disabled={!canRun}>
                    {starting ? (
                      <IconLoader2 size={14} className="animate-spin" />
                    ) : (
                      <IconPlayerPlay size={14} />
                    )}
                    {agentIds.length > 1
                      ? t('bench:run.runEach', { count: agentIds.length })
                      : t('bench:run.run')}
                  </Button>
                  {needsModel && !model.trim() && (
                    <span className="text-xs text-amber-600">
                      {t('bench:run.needsModel')}
                    </span>
                  )}
                </div>
              </div>
            </Card>
          </Section>

          {openRun && (
            <Section title={t('bench:detail.title')}>
              <Card className="rounded-lg bg-card">
                <RunDetail run={openRun} onChanged={() => open(openRun.id)} />
              </Card>
            </Section>
          )}

          {compareRuns.length > 1 && (
            <Section
              title={t('bench:compare.title')}
              actions={
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    setCompareIds([])
                    setCompareRuns([])
                  }}
                >
                  {t('bench:compare.clear')}
                </Button>
              }
            >
              <Card className="rounded-lg bg-card">
                <CompareTable runs={compareRuns} />
              </Card>
            </Section>
          )}

          <Section
            title={t('bench:history.title')}
            description={t('bench:history.description')}
          >
            <RunHistory
              runs={runs}
              openRunId={openRunId}
              compareIds={compareIds}
              onOpen={open}
              onToggleCompare={(id) => {
                toggleCompare(id).catch((err) => toast.error(String(err)))
              }}
            />
          </Section>
        </div>
      </div>

      <TaskEditorDialog
        open={!!editor}
        initial={editor?.initial ?? null}
        fixtures={overview?.fixtures ?? []}
        onClose={() => setEditor(null)}
        onSaved={async (saved) => {
          setEditor(null)
          await loadTasks()
          setTaskId(saved.id)
        }}
      />
    </div>
  )
}

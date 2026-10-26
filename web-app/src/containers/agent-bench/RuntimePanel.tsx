import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { toast } from 'sonner'
import {
  IconArrowBackUp,
  IconLoader2,
  IconRefresh,
} from '@tabler/icons-react'
import { Button } from '@/components/ui/button'
import { useTranslation } from '@/i18n/react-i18next-compat'
import { createSafeUnlisten } from '@/lib/tauriEvent'
import {
  RUNTIME_LOG_EVENT,
  RUNTIME_STATUS_EVENT,
  runtimeApi,
} from '@/lib/agent-bench/api'
import {
  shortCommit,
  type RuntimeStatus,
  type RuntimeStatusKind,
} from '@/lib/agent-bench/types'
import { cn } from '@/lib/utils'

const TONE: Record<RuntimeStatusKind, string> = {
  current: 'bg-emerald-500/15 text-emerald-700 dark:text-emerald-400',
  update_available: 'bg-sky-500/15 text-sky-700 dark:text-sky-400',
  refreshing: 'bg-sky-500/15 text-sky-700 dark:text-sky-400',
  pinned: 'bg-violet-500/15 text-violet-700 dark:text-violet-400',
  stale: 'bg-amber-500/15 text-amber-700 dark:text-amber-400',
  external: 'bg-amber-500/15 text-amber-700 dark:text-amber-400',
  update_failed: 'bg-destructive/15 text-destructive',
  not_installed: 'bg-muted text-muted-foreground',
}

const when = (iso: string | null | undefined) =>
  iso ? new Date(iso).toLocaleString() : null

/**
 * The stack-agent runtime Agent Bench runs: which revision, whether it is
 * the latest `main` (and on what evidence), and the refresh, rollback and
 * revision controls. Claude Code and other agents do not use it.
 */
export function RuntimePanel({
  initial,
}: {
  /** Status to show before the first fetch (tests, previews). */
  initial?: RuntimeStatus
}) {
  const { t } = useTranslation()
  const [status, setStatus] = useState<RuntimeStatus | null>(initial ?? null)
  const [busy, setBusy] = useState(false)
  const [log, setLog] = useState<string[]>([])
  const pending = useRef<ReturnType<typeof setTimeout> | null>(null)

  const load = useCallback(async () => {
    try {
      setStatus(await runtimeApi.status())
    } catch (err) {
      console.error('stack-agent runtime status', err)
    }
  }, [])

  useEffect(() => {
    if (!initial) load()
    const offStatus = createSafeUnlisten(
      listen(RUNTIME_STATUS_EVENT, () => {
        if (pending.current) clearTimeout(pending.current)
        pending.current = setTimeout(load, 250)
      })
    )
    const offLog = createSafeUnlisten(
      listen<string>(RUNTIME_LOG_EVENT, (e) =>
        setLog((l) => [...l.slice(-199), e.payload])
      )
    )
    return () => {
      offStatus()
      offLog()
      if (pending.current) clearTimeout(pending.current)
    }
  }, [initial, load])

  const act = async (fn: () => Promise<RuntimeStatus>, done?: string) => {
    setBusy(true)
    try {
      setStatus(await fn())
      if (done) toast.success(done)
    } catch (err) {
      toast.error(String(err))
    } finally {
      setBusy(false)
    }
  }

  if (!status)
    return (
      <p className="text-xs text-muted-foreground">
        {t('bench:runtime.loading')}
      </p>
    )

  const refreshing = status.status === 'refreshing'
  const pinned =
    status.selection.mode === 'pinned' ? status.selection.commit : null
  const canRollback =
    !pinned &&
    status.previous.some((c) =>
      status.cached.some((x) => x.commit === c && x.valid)
    )
  const valid = status.cached.filter((c) => c.valid && c.manifest)
  const resolved = status.resolved

  return (
    <div className="flex flex-col gap-2 text-xs" data-testid="runtime-panel">
      <div className="flex flex-wrap items-center gap-2">
        <span
          data-testid="runtime-status"
          className={cn(
            'inline-flex items-center gap-1 rounded-full px-2 py-0.5 font-medium',
            TONE[status.status]
          )}
        >
          {refreshing && <IconLoader2 size={12} className="animate-spin" />}
          {t(`bench:runtime.status.${status.status}`)}
        </span>
        <span className="min-w-0 font-mono">
          {resolved?.commit
            ? shortCommit(resolved.commit)
            : t('bench:runtime.noRevision')}
          {resolved?.version && (
            <span className="text-muted-foreground">
              {' '}
              · {resolved.version}
            </span>
          )}
        </span>
        <div className="ml-auto flex gap-1">
          <Button
            variant="ghost"
            size="sm"
            disabled={busy || refreshing}
            onClick={() => act(runtimeApi.refresh)}
          >
            <IconRefresh size={14} />
            {status.active || resolved
              ? t('bench:runtime.refresh')
              : t('bench:runtime.prepare')}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={busy || refreshing || !canRollback}
            title={canRollback ? undefined : t('bench:runtime.noRollback')}
            onClick={() =>
              act(runtimeApi.rollback, t('bench:runtime.rolledBack'))
            }
          >
            <IconArrowBackUp size={14} />
            {t('bench:runtime.rollback')}
          </Button>
        </div>
      </div>
      <p className="break-words text-muted-foreground">{status.detail}</p>
      {status.resolve_error && !resolved && (
        <p className="break-words text-destructive">{status.resolve_error}</p>
      )}
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5">
        <dt className="text-muted-foreground">{t('bench:runtime.source')}</dt>
        <dd className="break-words">
          {resolved
            ? t(`bench:runtime.sourceKind.${resolved.source}`)
            : t('bench:unavailable')}
        </dd>
        <dt className="text-muted-foreground">
          {t('bench:runtime.latestMain')}
        </dt>
        <dd className="break-words font-mono">
          {status.latest_remote
            ? `${shortCommit(status.latest_remote.commit)} (${when(status.latest_remote.checked_at)})`
            : t('bench:runtime.neverChecked')}
        </dd>
        <dt className="text-muted-foreground">
          {t('bench:runtime.lastRefresh')}
        </dt>
        <dd>
          {when(status.last_success_refresh_at) ?? t('bench:runtime.never')}
        </dd>
      </dl>
      {status.refreshing && (
        <p
          className="truncate font-mono text-muted-foreground"
          title={status.refreshing.last_line}
        >
          {status.refreshing.stage}
          {status.refreshing.last_line
            ? ` — ${status.refreshing.last_line}`
            : ''}
        </p>
      )}
      {valid.length > 0 && (
        <label className="flex flex-wrap items-center gap-2">
          <span className="text-muted-foreground">
            {t('bench:runtime.revision')}
          </span>
          <select
            aria-label={t('bench:runtime.revision')}
            className="h-7 min-w-0 max-w-full flex-1 rounded-md border bg-background px-1 font-mono text-xs"
            disabled={busy || refreshing}
            value={pinned ?? ''}
            onChange={(e) =>
              act(() =>
                runtimeApi.select(
                  e.target.value
                    ? { mode: 'pinned', commit: e.target.value }
                    : { mode: 'follow_main' }
                )
              )
            }
          >
            <option value="">{t('bench:runtime.followMain')}</option>
            {valid.map((c) => (
              <option key={c.commit} value={c.commit}>
                {t('bench:runtime.pinTo', {
                  commit: shortCommit(c.commit),
                  version: c.manifest!.version,
                })}
              </option>
            ))}
          </select>
        </label>
      )}
      {log.length > 0 && (
        <details>
          <summary className="cursor-pointer text-muted-foreground">
            {t('bench:runtime.log', { count: log.length })}
          </summary>
          <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-words rounded bg-muted p-2 text-[11px]">
            {log.join('\n')}
          </pre>
        </details>
      )}
    </div>
  )
}

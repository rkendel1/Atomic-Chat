import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { useTranslation } from '@/i18n/react-i18next-compat'
import { benchApi } from '@/lib/agent-bench/api'
import type { BenchTask, WorkshopRun } from '@/lib/agent-bench/types'

const slug = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^[-.]+|-+$/g, '')
    .slice(0, 80)

/**
 * Turns a finished run into a new, versioned regression task. The source task
 * and run are only read; the new task records where it came from.
 */
export function CreateRegressionDialog({
  run,
  onClose,
  onCreated,
}: {
  run: WorkshopRun | null
  onClose: () => void
  onCreated: (task: BenchTask) => void
}) {
  const { t } = useTranslation()
  const [id, setId] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [improvement, setImprovement] = useState('')
  const [expected, setExpected] = useState<'success' | 'failure'>('success')
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!run) return
    const base = `${run.task.id}-regression-${run.id.slice(-8)}`
    setId(slug(base))
    setName(`Regression: ${run.task.name}`)
    // A person writes this; the rules' summary is only shown as a reference.
    setDescription('')
    setImprovement('')
    setExpected('success')
  }, [run])

  if (!run) return null
  const primary = run.analysis?.causes.find((c) => c.primary)
  const workdir = run.task.snapshot.workdir

  const create = async () => {
    setSaving(true)
    try {
      const task = await benchApi.createRegression(run.id, {
        id: slug(id),
        name: name.trim() || null,
        failure_description: description.trim(),
        proposed_improvement: improvement.trim() || null,
        expected_outcome: expected,
      })
      toast.success(t('bench:regression.created', { id: task.id }))
      onCreated(task)
    } catch (err) {
      toast.error(t('bench:regression.failed'), { description: String(err) })
    } finally {
      setSaving(false)
    }
  }

  const label = 'text-xs font-medium text-muted-foreground'
  return (
    <Dialog open={!!run} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{t('bench:regression.title')}</DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-3 text-sm">
          <div className="rounded-md bg-muted p-2 text-xs">
            <p>
              {t('bench:regression.source', {
                task: run.task.id,
                version: run.task.version,
                run: run.id,
              })}
            </p>
            <p className="text-muted-foreground">
              {t('bench:regression.preserved')}
            </p>
            {primary && (
              <p className="mt-1">
                {t(`bench:category.${primary.category}`)} ·{' '}
                {t(`bench:confidence.${primary.confidence}`)}
              </p>
            )}
            <p className="mt-1 text-muted-foreground">
              {workdir.kind === 'path'
                ? run.baseline?.git_dirty === false && run.baseline.git_commit
                  ? t('bench:regression.snapshotClean', {
                      commit: run.baseline.git_commit.slice(0, 12),
                    })
                  : t('bench:regression.snapshotUnavailable')
                : t('bench:regression.sameBaseline')}
            </p>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <label className="flex flex-col gap-1">
              <span className={label}>{t('bench:editor.id')}</span>
              <Input value={id} onChange={(e) => setId(e.target.value)} />
            </label>
            <label className="flex flex-col gap-1">
              <span className={label}>{t('bench:editor.name')}</span>
              <Input value={name} onChange={(e) => setName(e.target.value)} />
            </label>
          </div>
          <label className="flex flex-col gap-1">
            <span className={label}>{t('bench:regression.description')}</span>
            <Textarea
              rows={3}
              value={description}
              placeholder={
                run.analysis?.summary
                  ? t('bench:regression.descriptionReference', {
                      summary: run.analysis.summary,
                    })
                  : t('bench:regression.descriptionPlaceholder')
              }
              onChange={(e) => setDescription(e.target.value)}
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className={label}>{t('bench:regression.improvement')}</span>
            <Textarea
              rows={2}
              value={improvement}
              placeholder={t('bench:regression.improvementPlaceholder')}
              onChange={(e) => setImprovement(e.target.value)}
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className={label}>{t('bench:regression.expected')}</span>
            <select
              className="h-9 w-64 rounded-md border bg-background px-2"
              value={expected}
              onChange={(e) =>
                setExpected(e.target.value as 'success' | 'failure')
              }
            >
              <option value="success">
                {t('bench:regression.expectSuccess')}
              </option>
              <option value="failure">
                {t('bench:regression.expectFailure')}
              </option>
            </select>
          </label>
        </div>
        <DialogFooter>
          <Button variant="secondary" onClick={onClose}>
            {t('common:cancel')}
          </Button>
          <Button
            onClick={create}
            disabled={saving || !slug(id) || !description.trim()}
          >
            {t('bench:regression.create')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

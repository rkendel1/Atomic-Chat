import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { IconFolder, IconPlus, IconTrash } from '@tabler/icons-react'
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
import { useServiceHub } from '@/hooks/useServiceHub'
import { useTranslation } from '@/i18n/react-i18next-compat'
import { benchApi } from '@/lib/agent-bench/api'
import { joinCommandLine, splitCommandLine } from '@/lib/agent-bench/args'
import {
  TASK_SCHEMA,
  type BenchTask,
  type VerificationCommand,
} from '@/lib/agent-bench/types'

type CommandDraft = { name: string; line: string; timeout: string }

const slug = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^[-.]+|-+$/g, '')
    .slice(0, 80)

/**
 * Create, edit or duplicate a task. Saving a changed task gives it the next
 * version; bundled tasks are read-only and open here only to duplicate.
 */
export function TaskEditorDialog({
  open,
  initial,
  fixtures,
  onClose,
  onSaved,
}: {
  open: boolean
  /** The task to edit, or to copy when it is bundled; none for a new task. */
  initial: BenchTask | null
  fixtures: string[]
  onClose: () => void
  onSaved: (task: BenchTask) => void
}) {
  const { t } = useTranslation()
  const serviceHub = useServiceHub()
  const editing = !!initial && !initial.builtin
  const [id, setId] = useState('')
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [goal, setGoal] = useState('')
  const [workdirKind, setWorkdirKind] = useState<'fixture' | 'path'>('path')
  const [fixture, setFixture] = useState('')
  const [path, setPath] = useState('')
  const [criteria, setCriteria] = useState('')
  const [method, setMethod] = useState<'command' | 'manual'>('command')
  const [commands, setCommands] = useState<CommandDraft[]>([])
  const [heldOut, setHeldOut] = useState<string | undefined>()
  const [instructions, setInstructions] = useState('')
  const [timeout, setTimeoutSecs] = useState('')
  const [maxTurns, setMaxTurns] = useState('')
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!open) return
    const src = initial
    setId(src ? (src.builtin ? `${src.id}-copy` : src.id) : '')
    setName(src ? (src.builtin ? `${src.name} (copy)` : src.name) : '')
    setDescription(src?.description ?? '')
    setGoal(src?.goal ?? '')
    setWorkdirKind(src?.workdir.kind ?? 'path')
    setFixture(
      src?.workdir.kind === 'fixture'
        ? src.workdir.fixture
        : (fixtures[0] ?? '')
    )
    setPath(src?.workdir.kind === 'path' ? src.workdir.path : '')
    setCriteria((src?.success_criteria ?? []).join('\n'))
    setMethod(src?.evaluation.method ?? 'command')
    setCommands(
      src?.evaluation.method === 'command'
        ? src.evaluation.commands.map((c) => ({
            name: c.name,
            line: joinCommandLine([c.program, ...c.args]),
            timeout: c.timeout_secs ? String(c.timeout_secs) : '',
          }))
        : [{ name: '', line: '', timeout: '' }]
    )
    setHeldOut(
      src?.evaluation.method === 'command'
        ? src.evaluation.held_out_checks
        : undefined
    )
    setInstructions(
      src?.evaluation.method === 'manual' ? src.evaluation.instructions : ''
    )
    setTimeoutSecs(
      src?.limits.timeout_secs ? String(src.limits.timeout_secs) : ''
    )
    setMaxTurns(src?.limits.max_turns ? String(src.limits.max_turns) : '')
  }, [open, initial, fixtures])

  const pickFolder = async () => {
    const selected = await serviceHub.dialog().open({
      multiple: false,
      directory: true,
    })
    if (typeof selected === 'string') setPath(selected)
  }

  const num = (v: string) => {
    const n = Number(v.trim())
    return v.trim() && Number.isFinite(n) && n > 0 ? Math.floor(n) : undefined
  }

  const save = async () => {
    const verification: VerificationCommand[] = commands
      .filter((c) => c.line.trim())
      .map((c) => {
        const [program = '', ...args] = splitCommandLine(c.line)
        return {
          name: c.name.trim() || c.line.trim(),
          program,
          args,
          ...(num(c.timeout) ? { timeout_secs: num(c.timeout) } : {}),
        }
      })
    const task: BenchTask = {
      schema: TASK_SCHEMA,
      id: editing ? initial!.id : slug(id || name),
      version: editing ? initial!.version : 1,
      name: name.trim(),
      description: description.trim(),
      goal: goal.trim(),
      workdir:
        workdirKind === 'fixture'
          ? { kind: 'fixture', fixture }
          : { kind: 'path', path: path.trim() },
      success_criteria: criteria
        .split('\n')
        .map((c) => c.trim())
        .filter(Boolean),
      evaluation:
        method === 'command'
          ? {
              method: 'command',
              commands: verification,
              // Held-out checks belong to the bundled fixture they came with.
              ...(heldOut && workdirKind === 'fixture'
                ? { held_out_checks: heldOut }
                : {}),
            }
          : { method: 'manual', instructions: instructions.trim() },
      limits: {
        ...(num(timeout) ? { timeout_secs: num(timeout) } : {}),
        ...(num(maxTurns) ? { max_turns: num(maxTurns) } : {}),
      },
      builtin: false,
      created_at: editing ? initial!.created_at : '',
      updated_at: editing ? initial!.updated_at : '',
    }
    setSaving(true)
    try {
      const saved = await benchApi.saveTask(task)
      toast.success(t('bench:editor.saved', { version: saved.version }))
      onSaved(saved)
    } catch (err) {
      toast.error(t('bench:editor.saveFailed'), { description: String(err) })
    } finally {
      setSaving(false)
    }
  }

  const label = 'text-xs font-medium text-muted-foreground'

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>
            {editing ? t('bench:editor.editTitle') : t('bench:editor.newTitle')}
          </DialogTitle>
        </DialogHeader>
        <div className="flex flex-col gap-3 text-sm">
          <div className="grid grid-cols-2 gap-3">
            <label className="flex flex-col gap-1">
              <span className={label}>{t('bench:editor.name')}</span>
              <Input value={name} onChange={(e) => setName(e.target.value)} />
            </label>
            <label className="flex flex-col gap-1">
              <span className={label}>{t('bench:editor.id')}</span>
              <Input
                value={editing ? initial!.id : id}
                disabled={editing}
                placeholder={slug(name)}
                onChange={(e) => setId(e.target.value)}
              />
            </label>
          </div>
          <label className="flex flex-col gap-1">
            <span className={label}>{t('bench:editor.description')}</span>
            <Input
              value={description}
              onChange={(e) => setDescription(e.target.value)}
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className={label}>{t('bench:editor.goal')}</span>
            <Textarea
              rows={4}
              value={goal}
              onChange={(e) => setGoal(e.target.value)}
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className={label}>{t('bench:editor.criteria')}</span>
            <Textarea
              rows={3}
              value={criteria}
              placeholder={t('bench:editor.criteriaPlaceholder')}
              onChange={(e) => setCriteria(e.target.value)}
            />
          </label>

          <div className="flex flex-col gap-1">
            <span className={label}>{t('bench:editor.project')}</span>
            <div className="flex items-center gap-2">
              <select
                className="h-9 rounded-md border bg-background px-2"
                value={workdirKind}
                onChange={(e) =>
                  setWorkdirKind(e.target.value as 'fixture' | 'path')
                }
              >
                <option value="path">{t('bench:editor.projectFolder')}</option>
                <option value="fixture">
                  {t('bench:editor.projectFixture')}
                </option>
              </select>
              {workdirKind === 'path' ? (
                <>
                  <Input
                    className="flex-1"
                    value={path}
                    placeholder="/path/to/project"
                    onChange={(e) => setPath(e.target.value)}
                  />
                  <Button variant="secondary" size="sm" onClick={pickFolder}>
                    <IconFolder size={14} />
                    {t('bench:editor.choose')}
                  </Button>
                </>
              ) : (
                <select
                  className="h-9 flex-1 rounded-md border bg-background px-2"
                  value={fixture}
                  onChange={(e) => setFixture(e.target.value)}
                >
                  {fixtures.map((f) => (
                    <option key={f} value={f}>
                      {f}
                    </option>
                  ))}
                </select>
              )}
            </div>
            <p className="text-[11px] text-muted-foreground">
              {workdirKind === 'path'
                ? t('bench:editor.projectFolderHint')
                : t('bench:editor.projectFixtureHint')}
            </p>
          </div>

          <div className="flex flex-col gap-1">
            <span className={label}>{t('bench:editor.evaluation')}</span>
            <select
              className="h-9 w-56 rounded-md border bg-background px-2"
              value={method}
              onChange={(e) =>
                setMethod(e.target.value as 'command' | 'manual')
              }
            >
              <option value="command">{t('bench:editor.methodCommand')}</option>
              <option value="manual">{t('bench:editor.methodManual')}</option>
            </select>
          </div>
          {method === 'command' ? (
            <div className="flex flex-col gap-2">
              <p className="text-[11px] text-muted-foreground">
                {t('bench:editor.commandsHint')}
              </p>
              {commands.map((c, i) => (
                <div key={i} className="flex items-center gap-2">
                  <Input
                    className="w-40"
                    value={c.name}
                    placeholder={t('bench:editor.commandName')}
                    onChange={(e) =>
                      setCommands((all) =>
                        all.map((x, j) =>
                          j === i ? { ...x, name: e.target.value } : x
                        )
                      )
                    }
                  />
                  <Input
                    className="flex-1 font-mono text-xs"
                    value={c.line}
                    placeholder="cargo test --offline"
                    onChange={(e) =>
                      setCommands((all) =>
                        all.map((x, j) =>
                          j === i ? { ...x, line: e.target.value } : x
                        )
                      )
                    }
                  />
                  <Input
                    className="w-20"
                    value={c.timeout}
                    placeholder={t('bench:editor.seconds')}
                    onChange={(e) =>
                      setCommands((all) =>
                        all.map((x, j) =>
                          j === i ? { ...x, timeout: e.target.value } : x
                        )
                      )
                    }
                  />
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    aria-label={t('bench:editor.removeCommand')}
                    onClick={() =>
                      setCommands((all) => all.filter((_, j) => j !== i))
                    }
                  >
                    <IconTrash size={14} />
                  </Button>
                </div>
              ))}
              <Button
                variant="ghost"
                size="sm"
                className="self-start"
                onClick={() =>
                  setCommands((all) => [
                    ...all,
                    { name: '', line: '', timeout: '' },
                  ])
                }
              >
                <IconPlus size={14} />
                {t('bench:editor.addCommand')}
              </Button>
              {heldOut && workdirKind === 'fixture' && (
                <p className="text-[11px] text-muted-foreground">
                  {t('bench:editor.heldOut', { set: heldOut })}
                </p>
              )}
            </div>
          ) : (
            <label className="flex flex-col gap-1">
              <span className={label}>{t('bench:editor.instructions')}</span>
              <Textarea
                rows={2}
                value={instructions}
                onChange={(e) => setInstructions(e.target.value)}
              />
            </label>
          )}

          <div className="grid grid-cols-2 gap-3">
            <label className="flex flex-col gap-1">
              <span className={label}>{t('bench:editor.timeout')}</span>
              <Input
                value={timeout}
                placeholder="900"
                onChange={(e) => setTimeoutSecs(e.target.value)}
              />
            </label>
            <label className="flex flex-col gap-1">
              <span className={label}>{t('bench:editor.maxTurns')}</span>
              <Input
                value={maxTurns}
                placeholder={t('bench:editor.agentDefault')}
                onChange={(e) => setMaxTurns(e.target.value)}
              />
            </label>
          </div>
        </div>
        <DialogFooter>
          <Button variant="secondary" onClick={onClose}>
            {t('common:cancel')}
          </Button>
          <Button
            onClick={save}
            disabled={saving || !name.trim() || !goal.trim()}
          >
            {t('bench:editor.save')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

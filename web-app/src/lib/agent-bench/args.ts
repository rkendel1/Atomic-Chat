/**
 * Verification commands are stored as a program and an argument list and run
 * without a shell. The editor shows them as one line; these convert between
 * the two. Double quotes group words; there is no other shell syntax.
 */
export function splitCommandLine(line: string): string[] {
  const out: string[] = []
  let current = ''
  let quoted = false
  let hasToken = false
  for (const ch of line) {
    if (ch === '"') {
      quoted = !quoted
      hasToken = true
    } else if (!quoted && /\s/.test(ch)) {
      if (hasToken) out.push(current)
      current = ''
      hasToken = false
    } else {
      current += ch
      hasToken = true
    }
  }
  if (hasToken) out.push(current)
  return out
}

export function joinCommandLine(parts: string[]): string {
  return parts
    .map((p) => (p === '' || /[\s"]/.test(p) ? `"${p.replace(/"/g, '')}"` : p))
    .join(' ')
}

/** `1 min 5 s`, `850 ms`; the caller decides what an absent value shows. */
export function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`
  const s = Math.round(ms / 1000)
  if (s < 60) return `${s} s`
  const m = Math.floor(s / 60)
  const rest = s % 60
  return rest ? `${m} min ${rest} s` : `${m} min`
}

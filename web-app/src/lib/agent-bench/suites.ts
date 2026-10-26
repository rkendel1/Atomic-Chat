/** `50% (1 of 2)` style rate; `null` when nothing was verified. */
export function formatRate(rate: number | null): string {
  if (rate == null) return '—'
  return `${Math.round(rate * 1000) / 10}%`
}

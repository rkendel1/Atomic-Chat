import { describe, expect, it } from 'vitest'
import { formatRate } from '../suites'

describe('formatRate', () => {
  it('shows no rate when nothing was verified', () => {
    expect(formatRate(null)).toBe('—')
  })
  it('rounds to one decimal', () => {
    expect(formatRate(0)).toBe('0%')
    expect(formatRate(1)).toBe('100%')
    expect(formatRate(2 / 3)).toBe('66.7%')
  })
})

import { describe, expect, it } from 'vitest'
import { formatDuration, joinCommandLine, splitCommandLine } from '../args'

describe('command lines', () => {
  it('splits on whitespace and keeps quoted words together', () => {
    expect(splitCommandLine('cargo test --offline')).toEqual([
      'cargo',
      'test',
      '--offline',
    ])
    expect(splitCommandLine('  npm   run "unit tests" ')).toEqual([
      'npm',
      'run',
      'unit tests',
    ])
    expect(splitCommandLine('echo ""')).toEqual(['echo', ''])
    expect(splitCommandLine('')).toEqual([])
  })

  it('round-trips through join', () => {
    const parts = ['pytest', '-k', 'fast and not slow', '']
    expect(splitCommandLine(joinCommandLine(parts))).toEqual(parts)
  })
})

describe('formatDuration', () => {
  it('picks a readable unit', () => {
    expect(formatDuration(850)).toBe('850 ms')
    expect(formatDuration(5_400)).toBe('5 s')
    expect(formatDuration(65_000)).toBe('1 min 5 s')
    expect(formatDuration(120_000)).toBe('2 min')
  })
})

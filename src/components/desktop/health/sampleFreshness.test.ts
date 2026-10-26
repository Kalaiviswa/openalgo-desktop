import { describe, expect, it } from 'vitest'
import { readTime } from './sampleFreshness'

describe('readTime', () => {
  const now = Date.parse('2026-10-23T09:30:00Z') // 3:00 pm IST, 23 Oct

  it('shows only the time for a moment today, in IST', () => {
    expect(readTime(Date.parse('2026-10-23T04:30:05Z'), now)).toBe('10:00:05 am IST')
  })

  // A gap open since yesterday read as one from this afternoon.
  it('adds the date for a moment on another day', () => {
    expect(readTime(Date.parse('2026-10-22T09:50:05Z'), now)).toBe('22 Oct, 03:20:05 pm IST')
  })

  it('takes the IST day, not the computer clock day', () => {
    // 23 Oct 00:10 IST is still 22 Oct in UTC.
    expect(readTime(Date.parse('2026-10-22T18:40:00Z'), now)).toBe('12:10:00 am IST')
  })

  it('says nothing was read', () => {
    expect(readTime(null, now)).toBe('-')
  })
})

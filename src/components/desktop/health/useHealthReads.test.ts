import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { CurrentMetrics, HeldOrder } from '@/api/health'
import { READ_TIMEOUT_MS, useHealthReads } from './useHealthReads'

const api = vi.hoisted(() => ({
  getCurrentMetrics: vi.fn(),
  getMetricsHistory: vi.fn(),
  getHealthStats: vi.fn(),
  getActiveAlerts: vi.fn(),
  getHeldOrders: vi.fn(),
}))

vi.mock('@/api/health', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/api/health')>()),
  ...api,
}))

const HELD: HeldOrder = {
  owner: 'strategy',
  id: '42',
  symbol: 'NIFTY07OCT2624500CE',
  exchange: 'NFO',
  action: 'BUY',
  quantity: 65,
  since: '2026-10-07 10:00:20',
  message: 'It may still be at the broker.',
}

describe('useHealthReads', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    api.getCurrentMetrics.mockRejectedValue(new Error('not needed'))
    api.getMetricsHistory.mockResolvedValue([])
    api.getHealthStats.mockRejectedValue(new Error('not needed'))
    api.getActiveAlerts.mockResolvedValue([])
  })

  it('keeps each answer on its own and counts the reads that failed', async () => {
    api.getHeldOrders.mockResolvedValue([HELD])
    const { result } = renderHook(() => useHealthReads(6, 24))

    let failed = -1
    await act(async () => {
      failed = await result.current.refresh()
    })
    expect(failed).toBe(2)
    expect(result.current.reads.held.data).toEqual([HELD])
    expect(result.current.reads.held.failedAt).toBeNull()
    expect(result.current.reads.current.data).toBeNull()
    expect(result.current.reads.current.failedAt).not.toBeNull()
  })

  // A slow earlier refresh landing after a newer one would put an order the
  // broker has since settled back on the page.
  it('applies only the latest refresh when an earlier one answers last', async () => {
    let answerFirst: (orders: HeldOrder[]) => void = () => {}
    api.getHeldOrders
      .mockReturnValueOnce(
        new Promise<HeldOrder[]>((resolve) => {
          answerFirst = resolve
        })
      )
      .mockResolvedValueOnce([])
    const { result } = renderHook(() => useHealthReads(6, 24))

    let first: Promise<number> = Promise.resolve(0)
    await act(async () => {
      first = result.current.refresh()
      await result.current.refresh()
    })
    expect(result.current.reads.held.data).toEqual([])
    await act(async () => {
      answerFirst([HELD])
      await first
    })
    expect(result.current.reads.held.data).toEqual([])
  })
})

/** A read that never answers and ignores its abort signal: a stalled server. */
const never = () => new Promise<never>(() => {})

// The five reads went through one Promise.allSettled: nothing was applied
// until the slowest had answered, so a held order's failed read waited on
// the slowest one, and a stalled server (the page's requests have no
// timeout) left the held list on screen unmarked.
describe('useHealthReads with a slow or stalled read', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.clearAllMocks()
    api.getMetricsHistory.mockResolvedValue([])
    api.getHealthStats.mockRejectedValue(new Error('not needed'))
    api.getActiveAlerts.mockResolvedValue([])
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('marks a failed read at once while another read is still waiting, each with its own time', async () => {
    let answerCurrent: (m: CurrentMetrics) => void = () => {}
    api.getCurrentMetrics.mockReturnValueOnce(
      new Promise<CurrentMetrics>((resolve) => {
        answerCurrent = resolve
      })
    )
    api.getHeldOrders.mockRejectedValueOnce(new Error('unreachable'))
    const { result } = renderHook(() => useHealthReads(6, 24))

    let refreshed: Promise<number> = Promise.resolve(-1)
    await act(async () => {
      refreshed = result.current.refresh()
    })
    expect(result.current.reads.held.failedAt).not.toBeNull()
    expect(result.current.reads.current.failedAt).toBeNull()
    expect(result.current.reads.current.readAt).toBeNull()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3_000)
      answerCurrent({ overall_status: 'pass' } as CurrentMetrics)
      expect(await refreshed).toBe(2)
    })
    const { current, held, history } = result.current.reads
    expect(current.data).not.toBeNull()
    expect(current.readAt).not.toBeNull()
    expect(history.readAt).not.toBeNull()
    expect((current.readAt ?? 0) - (history.readAt ?? 0)).toBeGreaterThanOrEqual(3_000)
    expect((current.readAt ?? 0) - (held.failedAt ?? 0)).toBeGreaterThanOrEqual(3_000)
  })

  it('gives up a stalled read, closes its request and marks it, keeping what it last read', async () => {
    api.getCurrentMetrics.mockRejectedValue(new Error('not needed'))
    api.getHeldOrders.mockResolvedValueOnce([HELD]).mockImplementationOnce(never)
    const { result } = renderHook(() => useHealthReads(6, 24))
    await act(async () => {
      await result.current.refresh()
    })
    expect(result.current.reads.held.data).toEqual([HELD])

    let refreshed: Promise<number> = Promise.resolve(-1)
    await act(async () => {
      refreshed = result.current.refresh()
      await vi.advanceTimersByTimeAsync(READ_TIMEOUT_MS - 1)
    })
    expect(result.current.reads.held.failedAt).toBeNull()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1)
      expect(await refreshed).toBe(3)
    })
    const held = result.current.reads.held
    expect(held.failedAt).not.toBeNull()
    expect(held.data).toEqual([HELD])
    const signal = api.getHeldOrders.mock.calls[1][0] as AbortSignal
    expect(signal.aborted).toBe(true)
  })

  it('closes the requests still waiting when the page is left, and applies none of them', async () => {
    api.getCurrentMetrics.mockImplementation(never)
    api.getHeldOrders.mockImplementation(never)
    const { result, unmount } = renderHook(() => useHealthReads(6, 24))
    await act(async () => {
      result.current.refresh()
    })
    unmount()
    for (const read of [api.getCurrentMetrics, api.getHeldOrders]) {
      expect((read.mock.calls[0][0] as AbortSignal).aborted).toBe(true)
    }
  })
})

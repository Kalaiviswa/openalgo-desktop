import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { HeldOrder } from '@/api/health'
import { useHealthReads } from './useHealthReads'

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

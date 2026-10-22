/**
 * Desktop: reading the orders held for the trader. An answer that does not
 * carry the list is a failed read; read as an empty list it told the
 * Health page that nothing was held (PROD-PAR-01).
 */

import { beforeEach, describe, expect, it, vi } from 'vitest'

const get = vi.fn()
vi.mock('./client', () => ({
  webClient: { get: (...a: unknown[]) => get(...a) },
}))

const { getHeldOrders } = await import('./health')

beforeEach(() => vi.clearAllMocks())

const held = {
  owner: 'strategy',
  id: '42',
  symbol: 'NIFTY07OCT2624500CE',
  exchange: 'NFO',
  action: 'BUY',
  quantity: 65,
  since: '2026-10-07 10:00:20',
  message: 'It may still be at the broker.',
}

describe('held orders', () => {
  it('returns the list the server sent, empty included', async () => {
    get.mockResolvedValueOnce({ data: { status: 'success', data: [held] } })
    expect(await getHeldOrders()).toEqual([held])
    get.mockResolvedValueOnce({ data: { status: 'success', data: [] } })
    expect(await getHeldOrders()).toEqual([])
  })

  it('fails on an answer without the list instead of returning none', async () => {
    get.mockResolvedValueOnce({ data: { status: 'success' } })
    await expect(getHeldOrders()).rejects.toThrow()
    get.mockResolvedValueOnce({ data: { status: 'error', message: 'Session expired' } })
    await expect(getHeldOrders()).rejects.toThrow()
    get.mockResolvedValueOnce({ data: { status: 'success', data: { orders: [held] } } })
    await expect(getHeldOrders()).rejects.toThrow()
  })
})

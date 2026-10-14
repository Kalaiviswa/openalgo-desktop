import { render, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'

/**
 * TOOLS-14: a contract whose previous-day OI could not be had has no OI
 * change (null from the server); the page leaves a gap there instead of
 * drawing a bar of zero (negating null gave -0 for the PE side).
 */

const mocks = vi.hoisted(() => ({
  api: {
    getUnderlyings: vi.fn(),
    getExpiries: vi.fn(),
    getIntervals: vi.fn(),
    getProfileData: vi.fn(),
  },
  plots: [] as unknown[][],
}))

vi.mock('@/api/oi-profile', () => ({ oiProfileApi: mocks.api }))
vi.mock('@/lib/Plot2D', () => ({
  default: (props: { data: unknown[] }) => {
    mocks.plots.push(props.data)
    return null
  },
}))
vi.mock('@/utils/toast', () => ({
  showToast: {
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
    dismissAll: vi.fn(),
    dynamic: vi.fn(),
    show: vi.fn(),
  },
}))
vi.mock('@/hooks/useSupportedExchanges', () => {
  const exchanges = [{ value: 'NFO', label: 'NFO' }]
  const value = {
    toolsFnoExchanges: exchanges,
    defaultToolsFnoExchange: 'NFO',
    fnoExchanges: exchanges,
    defaultFnoExchange: 'NFO',
    defaultUnderlyings: { NFO: ['NIFTY'] },
    isCrypto: false,
  }
  return { useSupportedExchanges: () => value }
})

beforeEach(() => {
  localStorage.clear()
  mocks.plots.length = 0
  mocks.api.getUnderlyings.mockResolvedValue({ status: 'success', underlyings: ['NIFTY'] })
  mocks.api.getExpiries.mockResolvedValue({ status: 'success', expiries: ['30-OCT-26'] })
  mocks.api.getIntervals.mockResolvedValue({ status: 'success', data: { intervals: ['5m'] } })
  mocks.api.getProfileData.mockResolvedValue({
    status: 'success',
    underlying: 'NIFTY',
    spot_price: 25010,
    atm_strike: 25000,
    lot_size: 75,
    expiry_date: '30OCT26',
    futures_symbol: 'NIFTY30OCT26FUT',
    interval: '5m',
    candles: [
      { time: 1791000000, timestamp: 1791000000, open: 1, high: 1, low: 1, close: 1, volume: 1 },
    ],
    oi_chain: [
      { strike: 24900, ce_oi: 1000, pe_oi: 4000, ce_oi_change: 200, pe_oi_change: 300 },
      { strike: 25000, ce_oi: 2000, pe_oi: 3000, ce_oi_change: null, pe_oi_change: null },
    ],
    message:
      "The daily OI change is shown for 2 of 4 option contracts: the broker's history did not give the previous day for 2.",
  })
})

describe('OI Profile', () => {
  it('leaves a gap where the OI change is unknown, not a bar of zero', async () => {
    const { default: Page } = await import('./OIProfile')
    render(
      <MemoryRouter>
        <Page />
      </MemoryRouter>
    )
    const changeTraces = () => {
      const last = mocks.plots.at(-1) as { name?: string; x?: unknown[] }[] | undefined
      return {
        ce: last?.find((t) => t.name === 'CE Change')?.x,
        pe: last?.find((t) => t.name === 'PE Change')?.x,
      }
    }
    await waitFor(() => expect(changeTraces().pe).toBeDefined())
    const { ce, pe } = changeTraces()
    expect(ce).toEqual([200, null])
    expect(pe?.[0]).toBe(-300)
    expect(pe?.[1]).toBeNull()
  })
})

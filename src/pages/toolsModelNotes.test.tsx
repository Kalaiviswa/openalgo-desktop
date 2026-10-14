import { render, screen } from '@testing-library/react'
import type { ComponentType } from 'react'
import { MemoryRouter } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'

/**
 * The chain tools say how their numbers are computed (TOOLS-15): the model,
 * the price it is computed against, the strikes it covers, and, on Gamma
 * Density, when its IV was assumed rather than taken from prices. The web
 * pages say none of this.
 */

const EXPIRY = '27-OCT-26'

const mocks = vi.hoisted(() => {
  const listing = () => ({ getUnderlyings: vi.fn(), getExpiries: vi.fn() })
  return {
    gex: { ...listing(), getGEXData: vi.fn() },
    gamma: { ...listing(), getGammaDensity: vi.fn() },
    ivSmile: { ...listing(), getIVSmileData: vi.fn() },
    oiTracker: { ...listing(), getOIData: vi.fn(), getMaxPain: vi.fn() },
  }
})

vi.mock('@/api/gex', () => ({ gexApi: mocks.gex }))
vi.mock('@/api/gamma-density', () => ({ gammaDensityApi: mocks.gamma }))
vi.mock('@/api/iv-smile', () => ({ ivSmileApi: mocks.ivSmile }))
vi.mock('@/api/oi-tracker', () => ({ oiTrackerApi: mocks.oiTracker }))
vi.mock('@/lib/Plot2D', () => ({ default: () => null }))
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

const STRIKES = [24900, 25000, 25100]

function gammaAnswer(extra: Record<string, unknown>) {
  return {
    status: 'success',
    underlying: 'NIFTY',
    exchange: 'NFO',
    expiry_date: '27OCT26',
    spot_price: 25000,
    forward_price: 25020,
    atm_strike: 25000,
    atm_iv: 15,
    dte_days: 13,
    interest_rate: 0,
    peak_intraday_strike: 25000,
    peak_expiry_strike: 25000,
    sigma_move: 196.28,
    one_sigma_low: 24803.72,
    one_sigma_high: 25196.28,
    two_sigma_low: 24607.43,
    two_sigma_high: 25392.57,
    chain: STRIKES.map((strike) => ({
      strike,
      ce_oi: 1000,
      pe_oi: 1000,
      iv: null,
      density_intraday: 1,
      density_expiry: 1,
    })),
    atm_iv_source: 'atm',
    estimated_iv_legs: 0,
    pricing_reference: 'forward',
    ...extra,
  }
}

async function renderPage(load: () => Promise<{ default: ComponentType }>) {
  const { default: Page } = await load()
  render(
    <MemoryRouter>
      <Page />
    </MemoryRouter>
  )
}

beforeEach(() => {
  localStorage.clear()
  for (const api of Object.values(mocks)) {
    for (const fn of Object.values(api)) fn.mockReset()
    api.getUnderlyings.mockResolvedValue({ status: 'success', underlyings: ['NIFTY'] })
    api.getExpiries.mockResolvedValue({ status: 'success', expiries: [EXPIRY] })
  }
})

describe('Gamma Density', () => {
  it('says when no price gave an IV and the 15% is assumed', async () => {
    mocks.gamma.getGammaDensity.mockResolvedValue(
      gammaAnswer({ atm_iv_source: 'assumed', estimated_iv_legs: 6 })
    )
    await renderPage(() => import('./GammaDensity'))
    const notice = await screen.findByTestId('gamma-assumed-iv')
    expect(notice).toHaveTextContent(
      'No option price in this chain gave an implied volatility, so the bands and densities use an assumed 15% IV. Treat them as estimates, not what the market implies.'
    )
    expect(screen.getByText('assumed, not from prices')).toBeInTheDocument()
    expect(screen.getByTestId('gamma-model-scope')).toHaveTextContent(
      'Densities cover the 3 strikes around the ATM strike shown here, not the whole option chain.'
    )
    // The assumption is said once, not again as borrowed legs.
    expect(screen.getByTestId('gamma-model-scope')).not.toHaveTextContent('no usable price')
  })

  it('names a median IV, the legs that borrowed it and a spot-priced chain', async () => {
    mocks.gamma.getGammaDensity.mockResolvedValue(
      gammaAnswer({ atm_iv_source: 'median', estimated_iv_legs: 2, pricing_reference: 'spot' })
    )
    await renderPage(() => import('./GammaDensity'))
    expect(await screen.findByText('median of strikes')).toBeInTheDocument()
    expect(screen.queryByTestId('gamma-assumed-iv')).toBeNull()
    expect(screen.getByTestId('gamma-model-scope')).toHaveTextContent(
      '2 options have no usable price and use the ATM IV.'
    )
    expect(
      screen.getByText(/on the spot price, because no synthetic future price was available/)
    ).toBeInTheDocument()
  })

  it('says nothing extra when the ATM strike priced its own IV', async () => {
    mocks.gamma.getGammaDensity.mockResolvedValue(gammaAnswer({}))
    await renderPage(() => import('./GammaDensity'))
    expect(await screen.findByTestId('gamma-model-scope')).toBeInTheDocument()
    expect(screen.queryByTestId('gamma-assumed-iv')).toBeNull()
    expect(screen.queryByText('median of strikes')).toBeNull()
    expect(screen.getByText(/on the synthetic future price/)).toBeInTheDocument()
  })
})

const chainRows = STRIKES.map((strike) => ({ strike, ce_oi: 1000, pe_oi: 2000 }))

const NOTES: {
  name: string
  load: () => Promise<{ default: ComponentType }>
  answer: () => void
  text: string
}[] = [
  {
    name: 'GEX Dashboard',
    load: () => import('./GEXDashboard'),
    answer: () =>
      mocks.gex.getGEXData.mockResolvedValue({
        status: 'success',
        underlying: 'NIFTY',
        spot_price: 25000,
        futures_price: 25020,
        lot_size: 75,
        atm_strike: 25000,
        expiry_date: '27OCT26',
        pcr_oi: 2,
        total_ce_oi: 3000,
        total_pe_oi: 6000,
        total_ce_gex: 10,
        total_pe_gex: 20,
        total_net_gex: -10,
        chain: chainRows.map((r) => ({
          ...r,
          ce_gamma: 0.001,
          pe_gamma: 0.001,
          ce_gex: 1,
          pe_gex: 2,
          net_gex: -1,
        })),
      }),
    text: 'GEX per strike is gamma × open interest × lot size, calls minus puts, with open interest as your broker reports it. Gamma uses the Black-76 model on the spot price',
  },
  {
    name: 'IV Smile',
    load: () => import('./IVSmile'),
    answer: () =>
      mocks.ivSmile.getIVSmileData.mockResolvedValue({
        status: 'success',
        underlying: 'NIFTY',
        spot_price: 25000,
        atm_strike: 25000,
        atm_iv: 14,
        skew: 1,
        expiry_date: '27OCT26',
        chain: STRIKES.map((strike) => ({ strike, ce_iv: 14, pe_iv: 15 })),
      }),
    text: 'Implied volatility uses the Black-76 model on the spot price',
  },
  {
    name: 'OI Tracker',
    load: () => import('./OITracker'),
    answer: () =>
      mocks.oiTracker.getOIData.mockResolvedValue({
        status: 'success',
        underlying: 'NIFTY',
        spot_price: 25000,
        futures_price: 25020,
        lot_size: 75,
        pcr_oi: 2,
        pcr_volume: 1,
        total_ce_oi: 3000,
        total_pe_oi: 6000,
        atm_strike: 25000,
        expiry_date: '27OCT26',
        chain: chainRows,
      }),
    text: 'Totals and PCR cover the 3 strikes around the ATM strike shown here, not the whole option chain.',
  },
  {
    name: 'Max Pain',
    load: () => import('./MaxPain'),
    answer: () =>
      mocks.oiTracker.getMaxPain.mockResolvedValue({
        status: 'success',
        underlying: 'NIFTY',
        spot_price: 25000,
        futures_price: 25020,
        atm_strike: 25000,
        max_pain_strike: 25000,
        lot_size: 75,
        pcr_oi: 2,
        pcr_volume: 1,
        expiry_date: '27OCT26',
        pain_data: STRIKES.map((strike) => ({
          strike,
          ce_pain: 1,
          pe_pain: 1,
          total_pain: 2,
          total_pain_cr: 0,
        })),
      }),
    text: 'Max pain is worked out over the 3 strikes around the ATM strike shown here, not the whole option chain',
  },
]

describe.each(NOTES)('$name', ({ load, answer, text }) => {
  it('says how its numbers are computed and which strikes they cover', async () => {
    answer()
    await renderPage(load)
    const note = await screen.findByTestId('tool-model-note')
    expect(note).toHaveTextContent(text)
    expect(note).toHaveTextContent('3 strikes around the ATM strike shown here')
  })
})

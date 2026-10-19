import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ComponentType } from 'react'
import { MemoryRouter } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { showToast } from '@/utils/toast'

/**
 * Only the answer for what the page shows now may change it (TOOLS-07,
 * TOOLS-08; gates T09 and T10). The trader asks for NIFTY, switches to
 * BANKNIFTY while NIFTY's answer is still on its way, and NIFTY's answer
 * then lands. The web pages took it: a chart, an error or the end of a
 * loading state that belonged to NIFTY appeared under BANKNIFTY.
 *
 * Every page reports an answer with `status: "error"` as a toast, so a
 * toast carrying NIFTY's message proves the page processed a stale answer;
 * a success answer goes through the same guard.
 */

const mocks = vi.hoisted(() => {
  const listing = () => ({ getUnderlyings: vi.fn(), getExpiries: vi.fn() })
  return {
    gex: { ...listing(), getGEXData: vi.fn() },
    gamma: { ...listing(), getGammaDensity: vi.fn() },
    ivSmile: { ...listing(), getIVSmileData: vi.fn() },
    oiTracker: { ...listing(), getOIData: vi.fn(), getMaxPain: vi.fn() },
    oiProfile: { ...listing(), getIntervals: vi.fn(), getProfileData: vi.fn() },
    ivChart: {
      ...listing(),
      getIntervals: vi.fn(),
      getIVData: vi.fn(),
      getDefaultSymbols: vi.fn(),
    },
    volSurface: { getSurfaceData: vi.fn() },
    straddle: { getIntervals: vi.fn(), getStraddleData: vi.fn() },
    custom: { getIntervals: vi.fn(), getLotSize: vi.fn(), simulate: vi.fn() },
    strikeProfile: { ...listing(), getProfileData: vi.fn() },
  }
})

vi.mock('@/api/gex', () => ({ gexApi: mocks.gex }))
vi.mock('@/api/gamma-density', () => ({ gammaDensityApi: mocks.gamma }))
vi.mock('@/api/iv-smile', () => ({ ivSmileApi: mocks.ivSmile }))
vi.mock('@/api/oi-tracker', () => ({ oiTrackerApi: mocks.oiTracker }))
vi.mock('@/api/oi-profile', () => ({ oiProfileApi: mocks.oiProfile }))
vi.mock('@/api/iv-chart', () => ({ ivChartApi: mocks.ivChart }))
vi.mock('@/api/vol-surface', () => ({ volSurfaceApi: mocks.volSurface }))
vi.mock('@/api/straddle-chart', () => ({ straddleChartApi: mocks.straddle }))
vi.mock('@/api/custom-straddle', () => ({ customStraddleApi: mocks.custom }))
vi.mock('@/api/strike-profile', () => ({ strikeProfileApi: mocks.strikeProfile }))
vi.mock('@/lib/Plot2D', () => ({ default: () => null }))
vi.mock('@/lib/Plot3D', () => ({ default: () => null }))
vi.mock('lightweight-charts', () => {
  // Every chart call answers with the same inert stub.
  const stub: unknown = new Proxy(() => stub, {
    get: (_target, key) => (key === 'then' ? undefined : stub),
  })
  return {
    createChart: () => stub,
    LineSeries: {},
    BaselineSeries: {},
    ColorType: { Solid: 'solid' },
    CrosshairMode: { Normal: 0 },
  }
})
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

type ApiMock = Record<string, ReturnType<typeof vi.fn>>

interface Deferred {
  resolve: (value: unknown) => void
}

/** Each call answers only when the test says so, in any order. */
function deferredCalls(fn: ReturnType<typeof vi.fn>): Deferred[] {
  const calls: Deferred[] = []
  fn.mockImplementation(
    () =>
      new Promise((resolve) => {
        calls.push({ resolve })
      })
  )
  return calls
}

interface PageCase {
  name: string
  load: () => Promise<{ default: ComponentType }>
  listing: ApiMock
  data: () => ReturnType<typeof vi.fn>
  /** The button that starts a load by hand, when the page does not load on its own. */
  start?: string
  /** The loading label of the page's own load button. */
  busy: string
}

const DATA_PAGES: PageCase[] = [
  {
    name: 'GEX Dashboard',
    load: () => import('./GEXDashboard'),
    listing: mocks.gex,
    data: () => mocks.gex.getGEXData,
    busy: 'Loading...',
  },
  {
    name: 'OI Tracker',
    load: () => import('./OITracker'),
    listing: mocks.oiTracker,
    data: () => mocks.oiTracker.getOIData,
    busy: 'Loading...',
  },
  {
    name: 'OI Range',
    load: () => import('./OIRange'),
    listing: mocks.oiTracker,
    data: () => mocks.oiTracker.getOIData,
    busy: 'Loading...',
  },
  {
    name: 'Max Pain',
    load: () => import('./MaxPain'),
    listing: mocks.oiTracker,
    data: () => mocks.oiTracker.getMaxPain,
    busy: 'Loading...',
  },
  {
    name: 'Gamma Density',
    load: () => import('./GammaDensity'),
    listing: mocks.gamma,
    data: () => mocks.gamma.getGammaDensity,
    busy: 'Loading...',
  },
  {
    name: 'IV Smile',
    load: () => import('./IVSmile'),
    listing: mocks.ivSmile,
    data: () => mocks.ivSmile.getIVSmileData,
    busy: 'Loading...',
  },
  {
    name: 'OI Profile',
    load: () => import('./OIProfile'),
    listing: mocks.oiProfile,
    data: () => mocks.oiProfile.getProfileData,
    busy: 'Loading...',
  },
  {
    name: 'Vol Surface',
    load: () => import('./VolSurface'),
    listing: mocks.oiProfile,
    data: () => mocks.volSurface.getSurfaceData,
    start: 'Load Surface',
    busy: 'Loading...',
  },
  {
    name: 'Strike Profile',
    load: () => import('./StrikeProfile'),
    listing: mocks.strikeProfile,
    data: () => mocks.strikeProfile.getProfileData,
    busy: 'Loading...',
  },
]

const CHART_PAGES: PageCase[] = [
  {
    name: 'Option Greeks',
    load: () => import('./IVChart'),
    listing: mocks.ivChart,
    data: () => mocks.ivChart.getIVData,
    busy: 'Loading...',
  },
  {
    name: 'Straddle Chart',
    load: () => import('./StraddleChart'),
    listing: mocks.oiProfile,
    data: () => mocks.straddle.getStraddleData,
    busy: 'Loading...',
  },
  {
    name: 'Straddle PnL',
    load: () => import('./CustomStraddle'),
    listing: mocks.oiProfile,
    data: () => mocks.custom.simulate,
    start: 'Simulate',
    busy: 'Simulating...',
  },
]

const NIFTY_ANSWER = { status: 'error', message: 'The answer for NIFTY' }
const BANKNIFTY_ANSWER = { status: 'error', message: 'The answer for BANKNIFTY' }

beforeEach(() => {
  localStorage.clear()
  for (const api of Object.values(mocks) as ApiMock[]) {
    for (const fn of Object.values(api)) fn.mockReset()
  }
  for (const api of [
    mocks.gex,
    mocks.gamma,
    mocks.ivSmile,
    mocks.oiTracker,
    mocks.oiProfile,
    mocks.ivChart,
    mocks.strikeProfile,
  ] as ApiMock[]) {
    api.getUnderlyings.mockResolvedValue({ status: 'success', underlyings: ['NIFTY', 'BANKNIFTY'] })
  }
  const intervals = { status: 'success', data: { intervals: ['1m', '5m'] } }
  mocks.oiProfile.getIntervals.mockResolvedValue(intervals)
  mocks.ivChart.getIntervals.mockResolvedValue(intervals)
  mocks.straddle.getIntervals.mockResolvedValue(intervals)
  mocks.custom.getIntervals.mockResolvedValue(intervals)
  mocks.custom.getLotSize.mockResolvedValue({ status: 'success', lotsize: 65 })
})

async function mount(page: PageCase) {
  const { default: Page } = await page.load()
  render(
    <MemoryRouter>
      <Page />
    </MemoryRouter>
  )
}

async function startLoad(page: PageCase, calls: number) {
  if (page.start) {
    const button = await screen.findByRole('button', { name: page.start })
    await waitFor(() => expect(button).toBeEnabled())
    fireEvent.click(button)
  }
  await waitFor(() => expect(page.data()).toHaveBeenCalledTimes(calls))
}

async function switchTo(underlying: string) {
  const trigger = screen
    .getAllByRole('combobox')
    .find((el) => el.textContent?.trim().startsWith('NIFTY'))
  if (!trigger) throw new Error('no underlying selector showing NIFTY')
  fireEvent.click(trigger)
  fireEvent.click(await screen.findByRole('option', { name: underlying }))
}

/** No request mixes one underlying with another's expiry. */
function expectNoMixedRequest(page: PageCase) {
  const expiryOf: Record<string, string> = { NIFTY: '27OCT26', BANKNIFTY: '28OCT26' }
  for (const [args] of page.data().mock.calls) {
    const a = args as { underlying?: string; expiry_date?: string; expiry_dates?: string[] }
    const expiry = a.expiry_date ?? a.expiry_dates?.[0]
    expect(`${a.underlying} ${expiry}`).toBe(`${a.underlying} ${expiryOf[a.underlying ?? '']}`)
  }
}

function toasted(message: string) {
  return vi.mocked(showToast.error).mock.calls.some((call) => call[0] === message)
}

describe.each(DATA_PAGES)('$name, discovery gap (T10)', (page) => {
  it.each([
    ['still running', () => new Promise(() => {})],
    ['failed', () => Promise.reject(new Error('Network Error'))],
  ])('drops the NIFTY answer while the BANKNIFTY lookup is %s', async (_label, bankniftyLookup) => {
    page.listing.getExpiries.mockImplementation((_exchange: string, underlying: string) =>
      underlying === 'NIFTY'
        ? Promise.resolve({ status: 'success', expiries: ['27-OCT-26'] })
        : bankniftyLookup()
    )
    const answers = deferredCalls(page.data())
    await mount(page)
    await startLoad(page, 1)
    await switchTo('BANKNIFTY')
    await act(async () => {
      answers[0].resolve(NIFTY_ANSWER)
    })
    expect(toasted(NIFTY_ANSWER.message)).toBe(false)
    expect(screen.queryByRole('button', { name: page.busy })).toBeNull()
    expectNoMixedRequest(page)
  })
})

describe.each(CHART_PAGES)('$name, answers out of order (T09)', (page) => {
  it('keeps the loading state and the result of the newest load only', async () => {
    page.listing.getExpiries.mockImplementation((_exchange: string, underlying: string) =>
      Promise.resolve({
        status: 'success',
        expiries: [underlying === 'NIFTY' ? '27-OCT-26' : '28-OCT-26'],
      })
    )
    const answers = deferredCalls(page.data())
    await mount(page)
    await startLoad(page, 1)
    await switchTo('BANKNIFTY')
    await screen.findByText('28-OCT-26')
    await startLoad(page, 2)
    // NIFTY's answer lands first, while BANKNIFTY's is still running.
    await act(async () => {
      answers[0].resolve(NIFTY_ANSWER)
    })
    expect(toasted(NIFTY_ANSWER.message)).toBe(false)
    expect(screen.getByRole('button', { name: page.busy })).toBeInTheDocument()
    await act(async () => {
      answers[1].resolve(BANKNIFTY_ANSWER)
    })
    expect(toasted(BANKNIFTY_ANSWER.message)).toBe(true)
    expect(screen.queryByRole('button', { name: page.busy })).toBeNull()
    expectNoMixedRequest(page)
  })

  it('drops an answer that lands after the page is left', async () => {
    page.listing.getExpiries.mockResolvedValue({ status: 'success', expiries: ['27-OCT-26'] })
    const answers = deferredCalls(page.data())
    const { default: Page } = await page.load()
    const view = render(
      <MemoryRouter>
        <Page />
      </MemoryRouter>
    )
    await startLoad(page, 1)
    view.unmount()
    await act(async () => {
      answers[0].resolve(NIFTY_ANSWER)
    })
    expect(toasted(NIFTY_ANSWER.message)).toBe(false)
  })
})

// Desktop addition (SM-03, SM-05): the Detail page attributes orders, trips
// and lifetime totals to Live or Sandbox by the run that made them.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { ReactNode } from 'react'
import { MemoryRouter, Route, Routes } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const rest = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))
const liveHook = vi.hoisted(() => vi.fn())

vi.mock('@/api/client', () => ({
  webClient: { get: rest.get, post: rest.post, patch: vi.fn(), delete: vi.fn() },
  apiClient: { post: vi.fn(), get: vi.fn() },
  authClient: { post: vi.fn() },
  fetchCSRFToken: vi.fn(),
  default: { post: vi.fn(), get: vi.fn() },
}))

vi.mock('@/api/strategy_module', async () => {
  const actual =
    await vi.importActual<typeof import('@/api/strategy_module')>('@/api/strategy_module')
  return { ...actual, useStrategyLive: liveHook }
})

vi.mock('@/utils/toast', () => ({
  showToast: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
}))

import type { StrategyLiveState } from '@/api/strategy_module'
import type { Run, RunMode, Strategy } from '@/types/strategy_module'
import StrategyDetail, { PositionsTab } from './Detail'
import type { ModedOrder, StrategyWithTotals } from './runModes'

const X = 'NIFTY28MAY2625000CE'
const Y = 'NIFTY28MAY2625500CE'

const strategy = {
  id: 7,
  name: 'Two modes',
  strategy_kind: 'batch',
  direction: 'both',
  universe_tab: 'weekly_monthly',
  underlying: 'NIFTY',
  underlying_exchange: 'NSE_INDEX',
  strategy_type: 'intraday',
  entry_time: null,
  exit_time: null,
  product: 'NRML',
  pricetype: 'MARKET',
  overall_sl_mtm: null,
  overall_target_mtm: null,
  lock_profit: null,
  trail_sl_to_entry: false,
  scheduler: null,
  live_enabled: true,
  webhook_locked: false,
  webhook_ip_allowlist: null,
  daily_loss_limit_inr: null,
  status: 'stopped',
  current_run_id: null,
  created_at: '2026-05-28T03:45:00+00:00',
  updated_at: '2026-05-28T03:45:00+00:00',
  legs: [],
} satisfies Strategy

const live = {
  status: 'idle',
  runId: null,
  checkpoint: null,
  legs: [],
  updatedAt: null,
  curve: [],
  isFetching: false,
  error: null,
  refresh: vi.fn(),
} satisfies StrategyLiveState

function run(id: number, mode: RunMode, pnl: number): Run {
  const minute = String(id % 60).padStart(2, '0')
  return {
    id,
    strategy_id: 7,
    mode,
    broker: null,
    started_at: `2026-05-${10 + Math.floor(id / 60)}T03:${minute}:00+00:00`,
    stopped_at: `2026-05-${10 + Math.floor(id / 60)}T03:${minute}:30+00:00`,
    stop_reason: 'manual',
    pnl_realized: pnl,
    pnl_peak: pnl,
    pnl_trough: pnl,
    trigger_source: 'manual',
    webhook_event_id: null,
    resolved_expiries: null,
  }
}

function fill(
  id: number,
  runId: number,
  mode: RunMode,
  symbol: string,
  kind: string,
  action: 'BUY' | 'SELL',
  price: number
): ModedOrder {
  return {
    id,
    run_id: runId,
    leg_id: 1,
    kind,
    broker_order_id: `B${id}`,
    position_ref: null,
    symbol,
    exchange: 'NFO',
    action,
    qty: 65,
    pricetype: 'MARKET',
    price: 0,
    trigger_price: 0,
    status: 'complete',
    placed_at: `2026-05-01T04:0${id % 10}:00+00:00`,
    filled_at: `2026-05-01T04:0${id % 10}:00+00:00`,
    avg_fill_price: price,
    filled_qty: 65,
    reject_reason: null,
    mode,
  }
}

function client() {
  return new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
}

function renderWithQuery(ui: ReactNode) {
  return render(<QueryClientProvider client={client()}>{ui}</QueryClientProvider>)
}

function renderDetail() {
  return render(
    <QueryClientProvider client={client()}>
      <MemoryRouter initialEntries={['/strategy/7']}>
        <Routes>
          <Route path="/strategy/:strategyId" element={<StrategyDetail />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  )
}

function card(title: string): HTMLElement {
  const element = screen.getByText(title).closest('[data-slot="card"]')
  expect(element).not.toBeNull()
  return element as HTMLElement
}

beforeEach(() => {
  vi.clearAllMocks()
  liveHook.mockReturnValue(live)
})

describe('Live and Sandbox attribution on the Detail page', () => {
  it('counts an older Live round trip as Live, not Sandbox (SM-05)', async () => {
    // The page loads at most 100 runs: 100 Sandbox runs, newer than the one
    // Live run whose round trip the orders still carry.
    const runs = Array.from({ length: 100 }, (_, i) => run(i + 2, 'sandbox', 0))
    const orders = [
      fill(1, 1, 'live', X, 'entry', 'BUY', 100),
      fill(2, 1, 'live', X, 'exit_manual', 'SELL', 110),
    ]
    rest.get.mockImplementation((url: string) => {
      if (url === '/strategy/api/strategies/7') {
        return Promise.resolve({ data: { data: strategy } })
      }
      if (url === '/strategy/api/strategies/7/runs') {
        return Promise.resolve({ data: { data: runs } })
      }
      if (url === '/strategy/api/strategies/7/orders') {
        return Promise.resolve({ data: { data: orders } })
      }
      return Promise.resolve({ data: { status: 'success', data: [] } })
    })
    const user = userEvent.setup()

    renderDetail()
    await user.click(await screen.findByRole('tab', { name: 'History' }))

    expect(await screen.findByRole('button', { name: /Live\s*\(1\)/ })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Sandbox\s*\(0\)/ })).toBeInTheDocument()
    const byMode = card('By mode')
    const liveCard = within(byMode).getByText('LIVE').closest('.rounded-md') as HTMLElement
    expect(liveCard).toHaveTextContent('1 trade')
    expect(liveCard).toHaveTextContent('+650.00')
  })

  it('takes the cumulative realized of the book mode from the server (SM-05)', () => {
    rest.get.mockResolvedValue({ data: { status: 'success', data: [] } })
    const withTotals = {
      ...strategy,
      realized_by_mode: { live: 250, sandbox: -616.5 },
    } satisfies StrategyWithTotals

    renderWithQuery(
      <PositionsTab
        strategy={withTotals}
        live={live}
        orders={[]}
        runs={[run(12, 'sandbox', 117), run(11, 'live', 250)]}
        loading={false}
        active
      />
    )

    const tile = screen.getByText('Cumulative realized').parentElement as HTMLElement
    expect(tile).toHaveTextContent('-616.50')
    expect(tile).toHaveTextContent('Lifetime across all Sandbox runs')
    // Not the page's own sum of the loaded runs, which mixed Live in.
    expect(tile).not.toHaveTextContent('367.00')
  })

  it('keeps a Sandbox residual out of the Live positions and lists it apart (SM-03)', async () => {
    // The broker book does not answer, so positions come from the orders.
    rest.get.mockResolvedValue({ data: { status: 'error', message: 'Broker session expired' } })
    const orders = [
      fill(1, 11, 'sandbox', X, 'entry', 'BUY', 100),
      fill(2, 12, 'live', Y, 'entry', 'BUY', 50),
    ]

    renderWithQuery(
      <PositionsTab
        strategy={strategy}
        live={live}
        orders={orders}
        runs={[run(12, 'live', 0), run(11, 'sandbox', 0)]}
        loading={false}
        active
      />
    )

    const book = card('Strategy positions')
    expect(await within(book).findByText(Y)).toBeInTheDocument()
    expect(within(book).queryByText(X)).toBeNull()
    const apart = card('Held by earlier Sandbox runs')
    expect(within(apart).getByText(X)).toBeInTheDocument()
    expect(within(apart).queryByText(Y)).toBeNull()
  })
})

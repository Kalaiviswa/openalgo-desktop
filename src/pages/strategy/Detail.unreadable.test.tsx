// Desktop addition (SM-04): the Detail page says when a strategy's records
// could not be read, instead of rendering empty tables.

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, waitFor } from '@testing-library/react'
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
import type { Strategy } from '@/types/strategy_module'
import StrategyDetail from './Detail'

const strategy = {
  id: 7,
  name: 'Unreadable records',
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
  live_enabled: false,
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

function renderDetail() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } })
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={['/strategy/7']}>
        <Routes>
          <Route path="/strategy/:strategyId" element={<StrategyDetail />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  )
}

/** Every read answers, except `failing`, which answers the server's 500. */
function serve(failing: string | null) {
  rest.get.mockImplementation((url: string) => {
    if (url === '/strategy/api/strategies/7') {
      return Promise.resolve({ data: { data: strategy } })
    }
    if (url === failing) {
      return Promise.reject(new Error('Request failed with status code 500'))
    }
    return Promise.resolve({ data: { data: [] } })
  })
}

describe('unreadable strategy records (SM-04)', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    liveHook.mockReturnValue(live)
  })

  it.each(['/strategy/api/strategies/7/orders', '/strategy/api/strategies/7/runs'])(
    'says the records could not be read when %s fails',
    async (failing) => {
      serve(failing)
      renderDetail()
      expect(await screen.findByRole('alert')).toHaveTextContent(
        "Some of this strategy's records could not be read"
      )
    }
  )

  it('shows no such notice when every record was read', async () => {
    serve(null)
    renderDetail()
    await screen.findByText('Unreadable records')
    await waitFor(() =>
      expect(rest.get).toHaveBeenCalledWith('/strategy/api/strategies/7/orders', expect.anything())
    )
    expect(screen.queryByText(/records could not be read/)).toBeNull()
  })
})

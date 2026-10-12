import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { PortfolioEntry } from '@/api/strategy-portfolio'
import StrategyPortfolio from './StrategyPortfolio'

/**
 * A saved watchlist that fails to load is not an empty watchlist (TOOLS-11,
 * gate T14). The web turned a failed request into `[]`, so a refresh could
 * replace saved strategies with "No strategies yet".
 */

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  remove: vi.fn(),
}))

vi.mock('@/api/strategy-portfolio', () => ({
  strategyPortfolioApi: { list: mocks.list, remove: mocks.remove },
}))
vi.mock('@/hooks/useMarketData', () => ({
  useMarketData: () => ({
    data: new Map(),
    isConnected: false,
    isPaused: false,
    isFallbackMode: false,
  }),
}))
vi.mock('@/utils/toast', () => ({
  showToast: { error: vi.fn(), success: vi.fn(), warning: vi.fn(), info: vi.fn() },
}))

function entry(id: number, name: string, watchlist: 'mytrades' | 'simulation'): PortfolioEntry {
  return {
    id,
    watchlist,
    name,
    underlying: 'NIFTY',
    exchange: 'NFO',
    expiry: '27-OCT-26',
    legs: [],
    notes: null,
    created_at: null,
    updated_at: null,
  }
}

const IRON = entry(1, 'Iron condor', 'mytrades')
const STRADDLE = entry(2, 'Short straddle', 'simulation')

function renderPage() {
  return render(
    <MemoryRouter>
      <StrategyPortfolio />
    </MemoryRouter>
  )
}

beforeEach(() => {
  mocks.list.mockReset()
  mocks.remove.mockReset()
})

describe('Strategy Portfolio lists', () => {
  it('says a watchlist failed instead of calling it empty, and Retry recovers', async () => {
    mocks.list
      .mockRejectedValueOnce(new Error('Network Error'))
      .mockResolvedValueOnce([STRADDLE])
      .mockResolvedValueOnce([IRON])
      .mockResolvedValueOnce([STRADDLE])
    renderPage()
    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent('Could not load your MyTrades strategies')
    expect(screen.queryByText('No strategies in MyTrades yet.')).toBeNull()
    fireEvent.click(within(alert).getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(screen.getByText('Iron condor')).toBeInTheDocument())
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('keeps the last loaded list, marked as such, when a refresh fails', async () => {
    mocks.list
      .mockResolvedValueOnce([IRON])
      .mockResolvedValueOnce([STRADDLE])
      .mockRejectedValueOnce(new Error('Network Error'))
      .mockResolvedValueOnce([])
    mocks.remove.mockResolvedValue(undefined)
    renderPage()
    await screen.findByText('Iron condor')
    // A delete in Simulation reloads both lists; MyTrades now fails.
    fireEvent.click(screen.getByRole('tab', { name: /Simulation/ }))
    fireEvent.mouseDown(screen.getByRole('tab', { name: /Simulation/ }))
    await waitFor(() => expect(screen.getByText('Short straddle')).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }))
    fireEvent.click(await screen.findByRole('button', { name: /^Delete/ }))
    await waitFor(() => expect(mocks.list).toHaveBeenCalledTimes(4))
    fireEvent.mouseDown(screen.getByRole('tab', { name: /MyTrades/ }))
    fireEvent.click(screen.getByRole('tab', { name: /MyTrades/ }))
    const alert = await screen.findByRole('alert')
    expect(alert).toHaveTextContent('The list below is from the last successful load')
    expect(screen.getByText('Iron condor')).toBeInTheDocument()
  })
})

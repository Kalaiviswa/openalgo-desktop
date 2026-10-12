import { act, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ArbitragePair } from '@/api/arbitrage'
import type { DataCallback, SymbolData } from '@/lib/MarketDataManager'
import Arbitrage, { computeRow, legsFresh, rankRows } from './Arbitrage'

/**
 * A calendar spread is live only when both legs are (TOOLS-12, gate T13).
 * The web took the newer leg's time, so a fresh near quote made a stale
 * far quote look executable.
 */

const PAIR: ArbitragePair = {
  id: 'NFO:NIFTY:near-next',
  underlying: 'NIFTY',
  exchange: 'NFO',
  type: 'near-next',
  near: {
    symbol: 'NIFTY27OCT26FUT',
    exchange: 'NFO',
    expiry: '27-OCT-26',
    lotsize: 65,
    tick_size: 0.1,
  },
  far: {
    symbol: 'NIFTY24NOV26FUT',
    exchange: 'NFO',
    expiry: '24-NOV-26',
    lotsize: 65,
    tick_size: 0.1,
  },
}

const NOW = 1_800_000_000_000
const q = (ts: number, extra: Record<string, unknown> = {}) => ({
  bid: 100,
  ask: 100.5,
  ltp: 100.2,
  ts,
  source: 'websocket' as const,
  epoch: 3,
  ...extra,
})

describe('legsFresh', () => {
  it('needs both legs recent and from the same session', () => {
    expect(legsFresh(q(NOW - 1000), q(NOW - 2000), NOW)).toBe(true)
    // One fresh leg, one stale, either way round.
    expect(legsFresh(q(NOW - 1000), q(NOW - 30_000), NOW)).toBe(false)
    expect(legsFresh(q(NOW - 30_000), q(NOW - 1000), NOW)).toBe(false)
    // Never observed.
    expect(legsFresh(q(NOW - 1000), q(0), NOW)).toBe(false)
    expect(legsFresh(q(NOW - 1000), undefined, NOW)).toBe(false)
    // Different WebSocket sessions.
    expect(legsFresh(q(NOW - 1000), q(NOW - 1000, { epoch: 2 }), NOW)).toBe(false)
    // One from REST, one from the socket.
    expect(legsFresh(q(NOW - 1000), q(NOW - 1000, { source: 'rest', epoch: undefined }), NOW)).toBe(
      false
    )
    // Both from REST polling is one session.
    const rest = { source: 'rest' as const, epoch: undefined }
    expect(legsFresh(q(NOW - 1000, rest), q(NOW - 1000, rest), NOW)).toBe(true)
  })

  it('ranks a stale spread below every live one, whatever its size', () => {
    const quotes = new Map([
      ['NFO:NIFTY27OCT26FUT', q(NOW - 1000, { bid: 100, ask: 100.5 })],
      ['NFO:NIFTY24NOV26FUT', q(NOW - 30_000, { bid: 150, ask: 150.5 })],
    ])
    const stale = computeRow(PAIR, quotes, NOW)
    expect(stale.fresh).toBe(false)
    expect(stale.nearAge).toBe(1000)
    expect(stale.farAge).toBe(30_000)
    const liveQuotes = new Map([
      ['NFO:NIFTY27OCT26FUT', q(NOW - 1000, { bid: 100, ask: 100.5 })],
      ['NFO:NIFTY24NOV26FUT', q(NOW - 1000, { bid: 101, ask: 101.5 })],
    ])
    const livePair = { ...PAIR, id: 'live' }
    const live = computeRow(livePair, liveQuotes, NOW)
    expect(live.fresh).toBe(true)
    expect((stale.spreadPct ?? 0) > (live.spreadPct ?? 0)).toBe(true)
    expect(rankRows([stale, live]).map((r) => r.pair.id)).toEqual(['live', PAIR.id])
  })
})

const callbacks = new Map<string, DataCallback>()
const fakeManager = {
  setAutoReconnect: vi.fn(),
  connect: vi.fn(),
  addStateListener: (listener: (s: unknown) => void) => {
    listener({ isConnected: true, isAuthenticated: true, isFallbackMode: false })
    return () => {}
  },
  subscribe: (symbol: string, exchange: string, _mode: string, cb: DataCallback) => {
    callbacks.set(`${exchange}:${symbol}`, cb)
    return () => callbacks.delete(`${exchange}:${symbol}`)
  },
  getCachedData: () => undefined,
}

vi.mock('@/contexts/MarketDataContext', () => ({
  useMarketDataContextOptional: () => ({ manager: fakeManager }),
}))
vi.mock('@/api/arbitrage', () => ({
  arbitrageApi: {
    getUniverse: vi.fn(async () => ({
      status: 'success',
      data: {
        pairs: [PAIR],
        symbols: [PAIR.near, PAIR.far].map(({ symbol, exchange }) => ({ symbol, exchange })),
        counts: { underlyings: 1, pairs: 1, symbols: 2 },
        generated_at: '2026-10-12T10:00:00',
      },
    })),
  },
}))

function tick(symbol: string, ageMs: number, bid: number): SymbolData {
  return {
    symbol,
    exchange: 'NFO',
    data: {
      ltp: bid + 0.2,
      depth: { buy: [{ price: bid, quantity: 65 }], sell: [{ price: bid + 0.5, quantity: 65 }] },
    },
    lastUpdate: Date.now() - ageMs,
    updateSource: 'websocket',
    connectionEpoch: 1,
  }
}

describe('Arbitrage page', () => {
  beforeEach(() => callbacks.clear())

  it('offers a spread to trade only while both legs are live', async () => {
    render(<Arbitrage />)
    await waitFor(() => expect(callbacks.size).toBe(2))
    act(() => {
      callbacks.get('NFO:NIFTY27OCT26FUT')?.(tick('NIFTY27OCT26FUT', 0, 100))
      callbacks.get('NFO:NIFTY24NOV26FUT')?.(tick('NIFTY24NOV26FUT', 30_000, 150))
    })
    const trade = await screen.findByRole('button', { name: 'Trade' }, { timeout: 3000 })
    await waitFor(() => expect(screen.getByText('100.00 / 100.50')).toBeInTheDocument(), {
      timeout: 3000,
    })
    expect(trade).toBeDisabled()
    act(() => {
      callbacks.get('NFO:NIFTY24NOV26FUT')?.(tick('NIFTY24NOV26FUT', 0, 101))
    })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Trade' })).toBeEnabled(), {
      timeout: 3000,
    })
  })
})

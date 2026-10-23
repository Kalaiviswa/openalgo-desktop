import { describe, expect, it } from 'vitest'
import type { CurrentMetrics } from '@/api/health'
import { checkA11y } from '@/test/a11y-utils'
import { render, screen } from '@/test/test-utils'
import { AppHealthCards } from './AppHealthCards'

function sample(extra: Partial<CurrentMetrics>): CurrentMetrics {
  return {
    timestamp: '2026-10-20T10:00:00+05:30',
    fd: { count: 1, limit: 10, usage_percent: 10, status: 'pass' },
    memory: { rss_mb: 1, vms_mb: 1, percent: 1, available_mb: 1, swap_mb: 0, status: 'pass' },
    database: { total: 1, connections: {}, status: 'pass' },
    websocket: { total: 0, connections: {}, total_symbols: 0, status: 'pass' },
    threads: { count: 1, stuck: 0, status: 'pass' },
    overall_status: 'pass',
    ...extra,
  }
}

const growth = {
  window_hours: 24,
  since: '2026-10-19T10:00:00+05:30',
  stores: {
    openalgo: { size_mb: 2.5, growth_mb: 0.1, per_day_mb: 0.1 },
    logs: { size_mb: 40, growth_mb: 6, per_day_mb: 6 },
    sandbox: { size_mb: 1, growth_mb: 0, per_day_mb: 0 },
    historify: { size_mb: 120, growth_mb: null, per_day_mb: null },
  },
  total: { size_mb: 163.5, growth_mb: 6.1, per_day_mb: 6.1 },
}

describe('App health cards', () => {
  it('shows each data file with its size and growth per day', () => {
    render(
      <AppHealthCards
        metrics={sample({ database: { total: 1, connections: {}, status: 'pass', growth } })}
      />
    )
    expect(screen.getByText('Data Files')).toBeInTheDocument()
    expect(screen.getByText('40.0 MB (+6.0 MB/day)')).toBeInTheDocument()
    expect(screen.getByText('120.0 MB')).toBeInTheDocument()
    expect(screen.getByText('163.5 MB (+6.1 MB/day)')).toBeInTheDocument()
  })

  it('says what was missed on the market data, per reader and to apps', () => {
    render(
      <AppHealthCards
        metrics={sample({
          market_data: {
            status: 'warn',
            broker_feeds: [
              {
                name: 'market_data',
                connected: true,
                connected_for_s: 60,
                last_update_age_s: 1,
                received_since_connect: 1200,
                skipped_since_connect: { strategy: 3, scalping: 0, feed_clients: 2 },
                skipped_total_since_connect: 5,
              },
            ],
            feed_clients: { coalesced: 10, dropped: 1, skipped: 0 },
          },
        })}
      />
    )
    expect(screen.getByText('Broker market data')).toBeInTheDocument()
    expect(screen.getByText('1,200 updates')).toBeInTheDocument()
    expect(screen.getByText('strategies 3, apps on the market data feed 2')).toBeInTheDocument()
    expect(screen.getByText('1 update missed')).toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveTextContent(/Some updates .* were skipped/)
    expect(screen.getByRole('status')).toHaveTextContent(/Apps on the market data feed missed/)
  })

  it('stays quiet when nothing was missed or lost', () => {
    render(
      <AppHealthCards
        metrics={sample({
          market_data: {
            status: 'pass',
            broker_feeds: [],
            feed_clients: { coalesced: 0, dropped: 0, skipped: 0 },
          },
          records: {
            status: 'pass',
            order_logs_dropped: 0,
            order_logs_failed: 0,
            monitoring_dropped: 0,
            monitoring_lost: 0,
            monitoring_retried_batches: 0,
          },
          strategy_ticks: {
            status: 'pass',
            ticks_queued: 50,
            ticks_folded: 0,
            ticks_missed: 0,
            exits_waiting: 0,
            oldest_exit_wait_s: null,
            exits_slow: false,
          },
        })}
      />
    )
    expect(screen.queryByRole('status')).not.toBeInTheDocument()
    expect(screen.getAllByText('all saved')).toHaveLength(2)
    expect(screen.getByText('Strategy Prices')).toBeInTheDocument()
    expect(screen.getByText('Strategies with exits waiting')).toBeInTheDocument()
    expect(screen.getByText('Longest exit wait').nextSibling).toHaveTextContent('none')
  })

  it('warns when a strategy exit has waited too long to be placed', () => {
    render(
      <AppHealthCards
        metrics={sample({
          strategy_ticks: {
            status: 'warn',
            ticks_queued: 50,
            ticks_folded: 0,
            ticks_missed: 0,
            exits_waiting: 2,
            oldest_exit_wait_s: 20,
            exits_slow: true,
          },
        })}
      />
    )
    expect(screen.getByText('Strategies with exits waiting').nextSibling).toHaveTextContent('2')
    expect(screen.getByText('20 s')).toBeInTheDocument()
    expect(screen.getByRole('status')).toHaveTextContent(
      /strategy exit decided 20 seconds ago is not placed yet.*Check that strategy's positions with your broker/
    )
    expect(screen.getByRole('status')).not.toHaveTextContent(/missed some prices/)
  })

  it('warns when strategy prices were missed and when order tracking has a gap', () => {
    render(
      <AppHealthCards
        metrics={sample({
          strategy_ticks: {
            status: 'warn',
            ticks_queued: 50,
            ticks_folded: 4,
            ticks_missed: 2,
            exits_waiting: 1,
            oldest_exit_wait_s: 3,
            exits_slow: false,
          },
          order_facts: {
            status: 'fail',
            gaps: [
              { mode: 'live', cause: 'order_feed_reconnected', since: '2026-10-20T04:30:00Z' },
            ],
            running: true,
            passes: 3,
            book_reads: 3,
            book_read_failures: 0,
            book_reads_discarded: 0,
            gaps_opened: 1,
            gaps_closed: 0,
            unconfirmed_orders: 0,
            bus: {
              published: 10,
              dropped: 0,
              critical_dropped: 0,
              closed_sends: 0,
              panics: 0,
              critical_panics: 0,
              stale_dropped: 0,
              relay_lagged: 0,
            },
          },
        })}
      />
    )
    const notes = screen.getAllByRole('status').map((n) => n.textContent)
    expect(notes).toContainEqual(
      expect.stringMatching(/Strategy risk management missed some prices/)
    )
    expect(notes).toContainEqual(
      expect.stringMatching(
        /the broker's order feed reconnected.*re-checking the broker's order book/
      )
    )
  })

  // Every card at once, warnings and the growth note included: rows only
  // inside each description list (a wrapper around a feed's rows, or a note
  // inside the list, is a serious axe finding).
  it('has no accessibility violations with every card and warning shown', async () => {
    const feed = (name: string, skipped: number) => ({
      name,
      connected: true,
      connected_for_s: 60,
      last_update_age_s: 1,
      received_since_connect: 100,
      skipped_since_connect: { strategy: skipped },
      skipped_total_since_connect: skipped,
    })
    const { container } = render(
      <AppHealthCards
        metrics={sample({
          database: {
            total: 1,
            connections: {},
            status: 'pass',
            growth: { ...growth, window_hours: 0.5 },
          },
          market_data: {
            status: 'warn',
            broker_feeds: [feed('market_data', 4), feed('order_updates', 1)],
            feed_clients: { coalesced: 0, dropped: 2, skipped: 0 },
          },
          records: {
            status: 'warn',
            order_logs_dropped: 1,
            order_logs_failed: 0,
            monitoring_dropped: 0,
            monitoring_lost: 0,
            monitoring_retried_batches: 0,
          },
          strategy_ticks: {
            status: 'warn',
            ticks_queued: 10,
            ticks_folded: 0,
            ticks_missed: 1,
            exits_waiting: 1,
            oldest_exit_wait_s: 20,
            exits_slow: true,
          },
        })}
      />
    )
    expect(screen.getByText(/Growth per day shows once/)).toBeInTheDocument()
    await checkA11y(container)
  })
})

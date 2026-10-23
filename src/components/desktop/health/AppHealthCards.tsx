/**
 * Desktop: what /health/api/current reports beyond the web's process
 * metrics, one compact card each: the data files and their growth, market
 * data missed per feed and on the way to apps, records not saved, the
 * prices strategy risk management read, and order tracking. A card turns
 * amber or red, with a sentence saying what it means, when something was
 * missed or lost.
 */

import { Fragment } from 'react'
import type {
  BrokerFeedLosses,
  CurrentMetrics,
  DataGrowth,
  MarketDataLosses,
  OrderFacts,
  RecordLosses,
  StrategyTicks,
} from '@/api/health'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { cn } from '@/lib/utils'

type Level = 'pass' | 'warn' | 'fail'

const STORE_NAMES: Record<string, string> = {
  openalgo: 'Main (symbols, orders, settings)',
  logs: 'Logs',
  sandbox: 'Sandbox',
  historify: 'Historify',
}

/** Who missed market data, as `feed_losses::reader_words` names them. */
const READER_NAMES: Record<string, string> = {
  strategy: 'strategies',
  scalping: 'scalping stops',
  sandbox: 'sandbox orders',
  feed_clients: 'apps on the market data feed',
  order_relay: 'order updates',
}

const FEED_NAMES: Record<string, string> = {
  market_data: 'Broker market data',
  order_updates: 'Broker order updates',
}

const GAP_CAUSES: Record<string, string> = {
  event_loss: 'an order update could not be delivered inside OpenAlgo',
  relay_lagged: "the broker's order updates came faster than OpenAlgo could relay them",
  order_feed_reconnected: "the broker's order feed reconnected",
}

function mb(v: number | null | undefined): string {
  return v == null ? '-' : `${v.toFixed(1)} MB`
}

function count(n: number, one: string, many: string): string {
  return `${n.toLocaleString('en-IN')} ${n === 1 ? one : many}`
}

/** A size, and its growth per day once there is a rate. */
function sizeAndGrowth(size: number | null | undefined, perDay: number | null | undefined) {
  return perDay != null ? `${mb(size)} (${perDay >= 0 ? '+' : ''}${mb(perDay)}/day)` : mb(size)
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between gap-2">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="font-medium text-right">{value}</dd>
    </div>
  )
}

/** A card of label and value rows (a description list, so only rows go in
 * it); `note` explains the figures, `warning` says what a miss means. */
function AppCard({
  title,
  level,
  note,
  warning,
  children,
}: {
  title: string
  level: Level
  note?: string | null
  warning?: string | null
  children: React.ReactNode
}) {
  return (
    <Card
      className={cn(
        level === 'warn' && 'border-yellow-500/50',
        level === 'fail' && 'border-red-500/50'
      )}
    >
      <CardHeader className="pb-3">
        <CardTitle className="text-base">{title}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <dl className="space-y-2 text-sm">{children}</dl>
        {note && <p className="text-xs text-muted-foreground">{note}</p>}
        {warning && (
          <p
            role="status"
            className={cn(
              'text-xs',
              level === 'fail'
                ? 'text-red-700 dark:text-red-400'
                : 'text-yellow-800 dark:text-yellow-400'
            )}
          >
            {warning}
          </p>
        )}
      </CardContent>
    </Card>
  )
}

export function DataFilesCard({
  sizes,
  growth,
}: {
  sizes?: Record<string, number> | null
  growth?: DataGrowth
}) {
  const names = Object.keys(STORE_NAMES)
  return (
    <AppCard
      title="Data Files"
      level="pass"
      note={
        growth && growth.window_hours < 1
          ? 'Growth per day shows once an hour of samples is kept.'
          : null
      }
    >
      {names.map((name) => {
        const store = growth?.stores[name]
        const size = store?.size_mb ?? sizes?.[name] ?? null
        return (
          <Row
            key={name}
            label={STORE_NAMES[name]}
            value={sizeAndGrowth(size, store?.per_day_mb)}
          />
        )
      })}
      <Row
        label="All data"
        value={sizeAndGrowth(growth?.total.size_mb, growth?.total.per_day_mb)}
      />
    </AppCard>
  )
}

function missedBy(feed: BrokerFeedLosses): string {
  return Object.entries(feed.skipped_since_connect)
    .filter(([, n]) => n > 0)
    .map(([reader, n]) => `${READER_NAMES[reader] ?? reader} ${n.toLocaleString('en-IN')}`)
    .join(', ')
}

export function MarketDataCard({ data }: { data: MarketDataLosses }) {
  const apps = data.feed_clients
  const appsMissed = apps.dropped + apps.skipped
  const feedMissed = data.broker_feeds.some((f) => f.skipped_total_since_connect > 0)
  const warning =
    data.status === 'warn'
      ? [
          feedMissed &&
            'Some updates since the feed connected were skipped because the part of OpenAlgo reading them had fallen behind. Each acts on the next update.',
          appsMissed > 0 &&
            'Apps on the market data feed missed some updates because they read too slowly.',
        ]
          .filter(Boolean)
          .join(' ')
      : null
  return (
    <AppCard title="Market Data" level={data.status} warning={warning}>
      {data.broker_feeds.length === 0 && <Row label="Broker feed" value="Not connected" />}
      {data.broker_feeds.map((feed) => (
        <Fragment key={feed.name}>
          <Row
            label={FEED_NAMES[feed.name] ?? feed.name}
            value={
              feed.connected
                ? count(feed.received_since_connect, 'update', 'updates')
                : 'Not connected'
            }
          />
          {feed.skipped_total_since_connect > 0 && <Row label="Missed by" value={missedBy(feed)} />}
        </Fragment>
      ))}
      <Row
        label="Apps on the feed"
        value={
          appsMissed > 0
            ? `${count(appsMissed, 'update', 'updates')} missed`
            : `none missed${apps.coalesced > 0 ? `, ${apps.coalesced.toLocaleString('en-IN')} replaced by a newer price` : ''}`
        }
      />
    </AppCard>
  )
}

export function RecordsCard({ data }: { data: RecordLosses }) {
  const orderLogs = data.order_logs_dropped + data.order_logs_failed
  const monitoring = data.monitoring_dropped + data.monitoring_lost
  return (
    <AppCard
      title="Records Saved"
      level={data.status}
      warning={
        data.status === 'warn'
          ? 'Some records were not saved, so a log page may miss entries since OpenAlgo started. Orders themselves were not affected.'
          : null
      }
    >
      <Row
        label="Order log entries"
        value={orderLogs > 0 ? `${orderLogs.toLocaleString('en-IN')} not saved` : 'all saved'}
      />
      <Row
        label="Monitoring records"
        value={monitoring > 0 ? `${monitoring.toLocaleString('en-IN')} not saved` : 'all saved'}
      />
      <Row label="Saves retried" value={data.monitoring_retried_batches.toLocaleString('en-IN')} />
    </AppCard>
  )
}

function strategyTicksWarning(data: StrategyTicks): string | null {
  const notes: string[] = []
  if (data.exits_slow && data.oldest_exit_wait_s !== null) {
    notes.push(
      `A strategy exit decided ${data.oldest_exit_wait_s.toLocaleString('en-IN')} seconds ago is not placed yet: the broker or OpenAlgo's database is answering slowly. Check that strategy's positions with your broker.`
    )
  }
  if (data.ticks_missed > 0) {
    notes.push(
      'Strategy risk management missed some prices: stops and targets act on the next price. Holding fewer symbols helps.'
    )
  }
  return notes.length > 0 ? notes.join(' ') : null
}

export function StrategyTicksCard({ data }: { data: StrategyTicks }) {
  return (
    <AppCard title="Strategy Prices" level={data.status} warning={strategyTicksWarning(data)}>
      <Row label="Prices read" value={data.ticks_queued.toLocaleString('en-IN')} />
      <Row label="Combined while busy" value={data.ticks_folded.toLocaleString('en-IN')} />
      <Row label="Missed" value={data.ticks_missed.toLocaleString('en-IN')} />
      <Row
        label="Strategies with exits waiting"
        value={data.exits_waiting.toLocaleString('en-IN')}
      />
      <Row
        label="Longest exit wait"
        value={
          data.oldest_exit_wait_s === null
            ? 'none'
            : `${data.oldest_exit_wait_s.toLocaleString('en-IN')} s`
        }
      />
    </AppCard>
  )
}

export function OrderTrackingCard({ data }: { data: OrderFacts }) {
  const lost = (data.bus?.critical_dropped ?? 0) + (data.bus?.critical_panics ?? 0)
  const gap = data.gaps[0]
  return (
    <AppCard
      title="Order Tracking"
      level={data.status}
      warning={
        gap
          ? `Some order updates may have been missed (${GAP_CAUSES[gap.cause] ?? gap.cause}). OpenAlgo is re-checking the broker's order book; new strategy entries wait until it has.`
          : null
      }
    >
      <Row label="Order book checks" value={data.running ? 'running' : 'stopped'} />
      <Row label="Order book reads" value={data.book_reads.toLocaleString('en-IN')} />
      <Row label="Reads that failed" value={data.book_read_failures.toLocaleString('en-IN')} />
      <Row label="Unconfirmed orders" value={data.unconfirmed_orders.toLocaleString('en-IN')} />
      <Row label="Order updates lost" value={lost.toLocaleString('en-IN')} />
    </AppCard>
  )
}

/** Every card the sample carries data for. */
export function AppHealthCards({ metrics }: { metrics: CurrentMetrics | null }) {
  if (!metrics) return null
  const { database, market_data, records, strategy_ticks, order_facts } = metrics
  return (
    <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
      {(database.growth || database.sizes_mb) && (
        <DataFilesCard sizes={database.sizes_mb} growth={database.growth} />
      )}
      {market_data && <MarketDataCard data={market_data} />}
      {order_facts && <OrderTrackingCard data={order_facts} />}
      {records && <RecordsCard data={records} />}
      {strategy_ticks && <StrategyTicksCard data={strategy_ticks} />}
    </div>
  )
}

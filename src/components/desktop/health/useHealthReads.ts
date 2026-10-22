/**
 * Desktop: the Health Monitor's reads, each kept on its own (PROD-PAR-01).
 *
 * The web reads its four answers with one Promise.all, so a single failed
 * read left every figure as it was with only a toast, and the desktop's
 * unconfirmed orders were read as an empty list whenever their read failed:
 * an order held for the trader vanished from the page after one failed
 * refresh. Here each read keeps its last answer; a failed refresh marks it
 * with the time it failed, and a read that never succeeded is "not read",
 * never empty. A failed read changes nothing on the server: an unconfirmed
 * order stays held until the trader marks it not placed.
 */

import { useCallback, useRef, useState } from 'react'
import {
  type CurrentMetrics,
  type EmptyHealthStats,
  getActiveAlerts,
  getCurrentMetrics,
  getHealthStats,
  getHeldOrders,
  getMetricsHistory,
  type HealthAlert,
  type HealthStats,
  type HeldOrder,
  type HistoricalMetric,
} from '@/api/health'

/** One read on the Health page. */
export interface HealthRead<T> {
  /** The last answer read; null until one was. */
  data: T | null
  /** When that answer was read (milliseconds since the epoch). */
  readAt: number | null
  /** When the latest refresh of it failed; null once one succeeds. */
  failedAt: number | null
}

export interface HealthReads {
  current: HealthRead<CurrentMetrics>
  history: HealthRead<HistoricalMetric[]>
  stats: HealthRead<HealthStats | EmptyHealthStats>
  alerts: HealthRead<HealthAlert[]>
  held: HealthRead<HeldOrder[]>
}

export const NOT_READ: HealthRead<never> = { data: null, readAt: null, failedAt: null }

const NOTHING_READ: HealthReads = {
  current: NOT_READ,
  history: NOT_READ,
  stats: NOT_READ,
  alerts: NOT_READ,
  held: NOT_READ,
}

/** A read's next state: the new answer, or the last one kept and marked. */
export function settle<T>(
  last: HealthRead<T>,
  result: PromiseSettledResult<T>,
  now: number
): HealthRead<T> {
  return result.status === 'fulfilled'
    ? { data: result.value, readAt: now, failedAt: null }
    : { ...last, failedAt: now }
}

/**
 * The page's reads and `refresh`, which reads them all again and resolves
 * to how many failed. It never rejects. Only the latest refresh's answers
 * are applied, so a slow earlier one cannot put an older list back.
 */
export function useHealthReads(chartHours: number, statsHours: number) {
  const [reads, setReads] = useState<HealthReads>(NOTHING_READ)
  const started = useRef(0)
  const applied = useRef(0)

  const refresh = useCallback(async (): Promise<number> => {
    const mine = ++started.current
    const results = await Promise.allSettled([
      getCurrentMetrics(),
      getMetricsHistory(chartHours),
      getHealthStats(statsHours),
      getActiveAlerts(),
      getHeldOrders(),
    ] as const)
    if (mine < applied.current) return 0
    applied.current = mine
    const [current, history, stats, alerts, held] = results
    const now = Date.now()
    setReads((last) => ({
      current: settle(last.current, current, now),
      history: settle(last.history, history, now),
      stats: settle(last.stats, stats, now),
      alerts: settle(last.alerts, alerts, now),
      held: settle(last.held, held, now),
    }))
    return results.filter((r) => r.status === 'rejected').length
  }, [chartHours, statsHours])

  return { reads, refresh }
}

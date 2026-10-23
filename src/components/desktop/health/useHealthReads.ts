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
 *
 * Each read is applied as soon as it settles, so one slow read never holds
 * back another's answer or its mark, and a read that has not answered by
 * `READ_TIMEOUT_MS` is given up (its request closed) and marked as failed:
 * the page's requests have no other timeout.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
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

/** How often the page reads everything again while Auto is on. */
export const REFRESH_INTERVAL_MS = 10_000

/**
 * A read still unanswered after this has failed. Under the refresh
 * interval, so a stalled read is marked before the next refresh asks again
 * and its requests never pile up behind a stalled server.
 */
export const READ_TIMEOUT_MS = 8_000

/** One read on the Health page. */
export interface HealthRead<T> {
  /** The last answer read; null until one was. */
  data: T | null
  /** When that answer was read (milliseconds since the epoch). */
  readAt: number | null
  /** When the latest refresh of it failed; null once one succeeds. */
  failedAt: number | null
}

/** What each read answers. */
interface Answers {
  current: CurrentMetrics
  history: HistoricalMetric[]
  stats: HealthStats | EmptyHealthStats
  alerts: HealthAlert[]
  held: HeldOrder[]
}

type ReadName = keyof Answers

export type HealthReads = { [K in ReadName]: HealthRead<Answers[K]> }

const NOT_READ: HealthRead<never> = { data: null, readAt: null, failedAt: null }

const NOTHING_READ: HealthReads = {
  current: NOT_READ,
  history: NOT_READ,
  stats: NOT_READ,
  alerts: NOT_READ,
  held: NOT_READ,
}

const NONE_STARTED: Record<ReadName, number> = {
  current: 0,
  history: 0,
  stats: 0,
  alerts: 0,
  held: 0,
}

/** A read's next state: the new answer, or the last one kept and marked. */
function settle<T>(
  last: HealthRead<T>,
  result: PromiseSettledResult<T>,
  now: number
): HealthRead<T> {
  return result.status === 'fulfilled'
    ? { data: result.value, readAt: now, failedAt: null }
    : { ...last, failedAt: now }
}

/**
 * `read`, given up after `READ_TIMEOUT_MS`: the request is aborted and the
 * answer is a failure even if the request does not stop at once.
 */
function withDeadline<T>(read: (signal: AbortSignal) => Promise<T>, abort: AbortController) {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      abort.abort()
      reject(new Error('The health read took too long'))
    }, READ_TIMEOUT_MS)
    read(abort.signal)
      .then(resolve, reject)
      .finally(() => clearTimeout(timer))
  })
}

/**
 * The page's reads and `refresh`, which reads them all again and resolves
 * to how many failed once every one has settled. It never rejects. Each
 * read is applied when it settles, unless a later refresh of the same read
 * was applied first, so a slow earlier answer cannot put an older list
 * back.
 */
export function useHealthReads(chartHours: number, statsHours: number) {
  const [reads, setReads] = useState<HealthReads>(NOTHING_READ)
  const started = useRef({ ...NONE_STARTED })
  const applied = useRef({ ...NONE_STARTED })
  const inFlight = useRef(new Set<AbortController>())
  // Counts the times the page was left. A read started before is closed
  // and never applied: not even as a failure (StrictMode leaves and comes
  // back at once in development).
  const left = useRef(0)

  useEffect(() => {
    const requests = inFlight.current
    return () => {
      left.current++
      for (const request of requests) request.abort()
      requests.clear()
    }
  }, [])

  /** One read, applied as soon as it settles; resolves to whether it succeeded. */
  const readOne = useCallback(
    async <K extends ReadName>(
      name: K,
      read: (signal: AbortSignal) => Promise<Answers[K]>
    ): Promise<boolean> => {
      const mine = ++started.current[name]
      const visit = left.current
      const abort = new AbortController()
      inFlight.current.add(abort)
      let result: PromiseSettledResult<Answers[K]>
      try {
        result = { status: 'fulfilled', value: await withDeadline(read, abort) }
      } catch (reason) {
        result = { status: 'rejected', reason }
      } finally {
        inFlight.current.delete(abort)
      }
      if (visit === left.current && mine > applied.current[name]) {
        applied.current[name] = mine
        const now = Date.now()
        setReads((last) => ({ ...last, [name]: settle(last[name], result, now) }))
      }
      return result.status === 'fulfilled'
    },
    []
  )

  const refresh = useCallback(async (): Promise<number> => {
    const succeeded = await Promise.all([
      readOne('current', (signal) => getCurrentMetrics(signal)),
      readOne('history', (signal) => getMetricsHistory(chartHours, signal)),
      readOne('stats', (signal) => getHealthStats(statsHours, signal)),
      readOne('alerts', (signal) => getActiveAlerts(signal)),
      readOne('held', (signal) => getHeldOrders(signal)),
    ])
    return succeeded.filter((ok) => !ok).length
  }, [chartHours, statsHours, readOne])

  return { reads, refresh }
}

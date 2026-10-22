/**
 * Desktop: whether the Health Monitor's latest sample still describes the
 * app. A missing or stale sample is UNKNOWN, never the PASS it last showed;
 * its readings stay on screen, in neutral colours. So is a sample the page
 * could not refresh (PROD-PAR-01): its figures stay, marked with when they
 * were read.
 */

import type { CurrentMetrics, HealthLevel } from '@/api/health'
import type { HealthRead } from './useHealthReads'

/** A sample older than two sampling periods no longer describes the app. */
const SAMPLE_STALE_AFTER_S = 120

const CLOCK = new Intl.DateTimeFormat('en-IN', {
  timeZone: 'Asia/Kolkata',
  hour: '2-digit',
  minute: '2-digit',
  second: '2-digit',
  hour12: true,
})

/** A time the page read or failed to read something, in IST. */
export function readTime(ms: number | null): string {
  return ms === null ? '-' : `${CLOCK.format(new Date(ms))} IST`
}

/** What the trader reads about a part of the page whose latest read failed. */
export function notRefreshedText(read: HealthRead<unknown>, what: string): string {
  return read.data === null
    ? `Could not read ${what}. Refresh to try again.`
    : `Could not refresh ${what} at ${readTime(read.failedAt)}; shown as read at ${readTime(read.readAt)}.`
}

/**
 * The sample's age now: as the server reported it when the page read it,
 * plus the time since, so a page that cannot refresh does not keep saying
 * "30 s ago". Else from its timestamp.
 */
function sampleAgeSeconds(
  metrics: CurrentMetrics,
  readAt: number | null,
  now: number
): number | null {
  if (typeof metrics.sample_age_s === 'number') {
    const sinceRead = readAt === null ? 0 : Math.max(0, Math.round((now - readAt) / 1000))
    return metrics.sample_age_s + sinceRead
  }
  const taken = Date.parse(metrics.timestamp)
  if (Number.isNaN(taken)) return null
  return Math.max(0, Math.round((now - taken) / 1000))
}

export function formatAge(seconds: number): string {
  if (seconds < 120) return `${seconds} s ago`
  if (seconds < 7200) return `${Math.floor(seconds / 60)} min ago`
  return `${Math.floor(seconds / 3600)} h ago`
}

export interface SampleFreshness {
  /** The overall status to show: the sample's, or `unknown` when stale. */
  overallStatus: HealthLevel
  /** Seconds since the sample was taken, when known. */
  sampleAge: number | null
  /** Why the status is unknown, for the trader; `null` when fresh. */
  staleReason: string | null
  /** A card's status to show: its own, or `unknown` when stale. */
  cardStatus: (status: HealthLevel | undefined) => HealthLevel
}

export function sampleFreshness(
  read: HealthRead<CurrentMetrics>,
  now: number = Date.now()
): SampleFreshness {
  const currentMetrics = read.data
  const sampleAge = currentMetrics ? sampleAgeSeconds(currentMetrics, read.readAt, now) : null
  const notRefreshed = read.failedAt !== null
  const stale =
    notRefreshed ||
    (currentMetrics !== null &&
      (currentMetrics.overall_status === 'unknown' ||
        sampleAge === null ||
        sampleAge > SAMPLE_STALE_AFTER_S))
  const overallStatus = stale ? 'unknown' : (currentMetrics?.overall_status ?? 'unknown')
  const staleReason = !stale
    ? null
    : notRefreshed
      ? notRefreshedText(read, 'health')
      : (currentMetrics?.reason ??
        `No health sample${sampleAge === null ? '' : ` for ${formatAge(sampleAge).replace(' ago', '')}`}. Health sampling may have stopped; restart OpenAlgo if this continues.`)
  const cardStatus = (status: HealthLevel | undefined): HealthLevel =>
    stale ? 'unknown' : status || 'unknown'
  return { overallStatus, sampleAge, staleReason, cardStatus }
}

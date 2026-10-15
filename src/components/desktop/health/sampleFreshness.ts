/**
 * Desktop: whether the Health Monitor's latest sample still describes the
 * app. A missing or stale sample is UNKNOWN, never the PASS it last showed;
 * its readings stay on screen, in neutral colours.
 */

import type { CurrentMetrics, HealthLevel } from '@/api/health'

/** A sample older than two sampling periods no longer describes the app. */
const SAMPLE_STALE_AFTER_S = 120

/** The sample's age as the server reports it, else from its timestamp. */
function sampleAgeSeconds(metrics: CurrentMetrics): number | null {
  if (typeof metrics.sample_age_s === 'number') return metrics.sample_age_s
  const taken = Date.parse(metrics.timestamp)
  if (Number.isNaN(taken)) return null
  return Math.max(0, Math.round((Date.now() - taken) / 1000))
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

export function sampleFreshness(currentMetrics: CurrentMetrics | null): SampleFreshness {
  const sampleAge = currentMetrics ? sampleAgeSeconds(currentMetrics) : null
  const stale =
    currentMetrics !== null &&
    (currentMetrics.overall_status === 'unknown' ||
      sampleAge === null ||
      sampleAge > SAMPLE_STALE_AFTER_S)
  const overallStatus = stale ? 'unknown' : (currentMetrics?.overall_status ?? 'unknown')
  const staleReason = stale
    ? (currentMetrics?.reason ??
      `No health sample${sampleAge === null ? '' : ` for ${formatAge(sampleAge).replace(' ago', '')}`}. Health sampling may have stopped; restart OpenAlgo if this continues.`)
    : null
  const cardStatus = (status: HealthLevel | undefined): HealthLevel =>
    stale ? 'unknown' : status || 'unknown'
  return { overallStatus, sampleAge, staleReason, cardStatus }
}

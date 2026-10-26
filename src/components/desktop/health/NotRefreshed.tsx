import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { notRefreshedText, readTime } from './sampleFreshness'
import type { HealthRead } from './useHealthReads'

/**
 * Desktop: a line saying that a part of the Health page could not be
 * refreshed (its last answer stays on screen) or was never read
 * (PROD-PAR-01). Nothing while the latest read succeeded.
 */
export function NotRefreshed({ read, what }: { read: HealthRead<unknown>; what: string }) {
  if (read.failedAt === null) return null
  // Also on the status banner's tint, where the muted grey is below 4.5:1.
  return (
    <p className="text-sm text-neutral-600 dark:text-muted-foreground">
      {notRefreshedText(read, what)}
    </p>
  )
}

/**
 * Desktop: a card saying that a part of the Health page whose own cards
 * have no header to carry the mark (the statistics) could not be refreshed
 * or was never read; or, for a list the page shows only when it has rows
 * (the alerts), that its latest read failed with no rows to mark: it was
 * never read, or was empty when last read. Never an empty list. A list
 * with rows on screen carries its own mark instead.
 */
export function NotRefreshedCard({
  title,
  read,
  what,
}: {
  title: string
  read: HealthRead<unknown>
  what: string
}) {
  const rows = Array.isArray(read.data) ? read.data.length : null
  if (read.failedAt === null || (rows !== null && rows > 0)) return null
  return (
    <Card className="border-yellow-500/50">
      <CardHeader className="pb-3">
        <CardTitle className="text-lg">{title}</CardTitle>
      </CardHeader>
      <CardContent className="text-sm text-muted-foreground">
        {rows === 0
          ? `Could not refresh ${what} at ${readTime(read.failedAt)}. There were none when last read at ${readTime(read.readAt)}.`
          : notRefreshedText(read, what)}
      </CardContent>
    </Card>
  )
}

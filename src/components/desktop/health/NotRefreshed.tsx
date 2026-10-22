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
 * Desktop: a card for a list the page shows only when it has rows (the
 * alerts), when its latest read failed and there are no rows to mark: it
 * was never read, or was empty when last read. Not an empty list.
 */
export function NotRefreshedCard({
  title,
  read,
  what,
}: {
  title: string
  read: HealthRead<unknown[]>
  what: string
}) {
  if (read.failedAt === null || (read.data !== null && read.data.length > 0)) return null
  return (
    <Card className="border-yellow-500/50">
      <CardHeader className="pb-3">
        <CardTitle className="text-lg">{title}</CardTitle>
      </CardHeader>
      <CardContent className="text-sm text-muted-foreground">
        {read.data === null
          ? notRefreshedText(read, what)
          : `Could not refresh ${what} at ${readTime(read.failedAt)}. There were none when last read at ${readTime(read.readAt)}.`}
      </CardContent>
    </Card>
  )
}

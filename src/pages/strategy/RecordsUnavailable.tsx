// pages/strategy/RecordsUnavailable.tsx
// Desktop addition to the web's Detail page (SM-04). The server answers a
// failed read of a strategy's local records with an error, where the web's
// store answers an empty success; without this notice an empty table would
// stand in for "no orders" or "never ran" when the records were unreadable.

export function RecordsUnavailable({ failed }: { failed: boolean }) {
  if (!failed) return null
  return (
    <p role="alert" className="rounded-md bg-destructive/10 p-3 text-sm text-destructive">
      Some of this strategy's records could not be read, so the orders, trades, positions, events or
      history below may be incomplete. Reload the page; if it keeps happening, restart the app.
    </p>
  )
}

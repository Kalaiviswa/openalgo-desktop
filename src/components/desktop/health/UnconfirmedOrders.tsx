import { useState } from 'react'
import { type HeldOrder, markHeldOrderNotPlaced } from '@/api/health'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { showToast } from '@/utils/toast'

const heldKey = (order: HeldOrder) => `${order.owner}:${order.id}`

/**
 * Desktop: live orders the broker did not confirm and does not show, held
 * for the trader's decision (P0-3). Marking one not placed, after a
 * confirmation, lets its position be managed again; `onResolved` then
 * refreshes the page.
 */
export function UnconfirmedOrders({
  orders,
  onResolved,
}: {
  orders: HeldOrder[]
  onResolved: () => void
}) {
  const [confirmingHeld, setConfirmingHeld] = useState<string | null>(null)

  const handleNotPlaced = async (order: HeldOrder) => {
    try {
      await markHeldOrderNotPlaced(order.owner, order.id)
      showToast.success('Marked as not placed. The position is managed again.', 'monitoring')
      setConfirmingHeld(null)
      onResolved()
    } catch (_error) {
      showToast.error(
        'Could not mark the order as not placed. Refresh the page and try again.',
        'monitoring'
      )
    }
  }

  if (orders.length === 0) return null
  return (
    <Card className="border-destructive/50">
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-destructive">
          Unconfirmed Orders ({orders.length})
        </CardTitle>
        <CardDescription>
          The broker did not confirm these orders and does not show them. They are not sent again.
          Check the broker's order book and trade book; if an order is not there, mark it as not
          placed to resume.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="space-y-3">
          {orders.map((order) => (
            <div
              key={heldKey(order)}
              className="rounded-lg border border-red-500/50 bg-red-500/10 p-3 flex items-start justify-between gap-4"
            >
              <div>
                <p className="font-medium">
                  {order.action} {order.quantity} {order.symbol} ({order.exchange})
                </p>
                <p className="text-sm text-muted-foreground">{order.message}</p>
                <p className="text-xs text-muted-foreground">Since {order.since}</p>
              </div>
              {confirmingHeld === heldKey(order) ? (
                <div className="flex gap-2">
                  <Button variant="destructive" size="sm" onClick={() => handleNotPlaced(order)}>
                    Confirm not placed
                  </Button>
                  <Button variant="outline" size="sm" onClick={() => setConfirmingHeld(null)}>
                    Cancel
                  </Button>
                </div>
              ) : (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setConfirmingHeld(heldKey(order))}
                >
                  Not placed
                </Button>
              )}
            </div>
          ))}
        </div>
      </CardContent>
    </Card>
  )
}

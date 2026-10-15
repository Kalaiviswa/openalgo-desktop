// pages/strategy/OtherModePositions.tsx
// Desktop addition to the web's Detail page (SM-03). The Positions tab shows
// the book of the run it is looking at, Live or Sandbox, and attributes to it
// only that mode's orders. A position this strategy's runs of the other mode
// still hold is listed here instead, apart from that book and its totals.

import { useMemo } from 'react'
import { derivePositions } from '@/api/strategy_module'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import type { Order, RunMode } from '@/types/strategy_module'
import { modeLabel, ordersOfMode, type RunModeIndex } from './runModes'

export function OtherModePositions({
  orders,
  modes,
  viewMode,
  product,
}: {
  orders: Order[]
  modes: RunModeIndex
  /** The mode of the book shown above; null when it is not known. */
  viewMode: RunMode | null
  product: string
}) {
  const otherMode: RunMode | null =
    viewMode === null ? null : viewMode === 'live' ? 'sandbox' : 'live'
  const held = useMemo(
    () =>
      otherMode === null
        ? []
        : derivePositions(ordersOfMode(orders, modes, otherMode), product).filter(
            (row) => row.net_qty !== 0
          ),
    [orders, modes, otherMode, product]
  )
  if (otherMode === null || viewMode === null || held.length === 0) return null
  return (
    <Card>
      <CardHeader>
        <CardTitle>Held by earlier {modeLabel(otherMode)} runs</CardTitle>
        <CardDescription>
          These positions come from this strategy's {modeLabel(otherMode)} runs, so they are not
          part of the {modeLabel(viewMode)} book above or its totals. Derived from the strategy's
          filled orders.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Symbol</TableHead>
                <TableHead>Exchange</TableHead>
                <TableHead>Product</TableHead>
                <TableHead>Mode</TableHead>
                <TableHead>Side</TableHead>
                <TableHead className="text-right">Net Qty</TableHead>
                <TableHead className="text-right">Avg Entry</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {held.map((row) => (
                <TableRow key={`${row.symbol}-${row.exchange}-${row.product}`}>
                  <TableCell className="font-mono font-medium">{row.symbol}</TableCell>
                  <TableCell className="font-mono text-xs">{row.exchange}</TableCell>
                  <TableCell>
                    <Badge variant="outline" className="text-xs">
                      {row.product}
                    </Badge>
                  </TableCell>
                  <TableCell>
                    <Badge variant={otherMode === 'live' ? 'destructive' : 'secondary'}>
                      {modeLabel(otherMode)}
                    </Badge>
                  </TableCell>
                  <TableCell>{row.side}</TableCell>
                  <TableCell className="text-right font-mono">{row.net_qty}</TableCell>
                  <TableCell className="text-right font-mono">
                    {row.avg_entry_price === null ? 'Unavailable' : row.avg_entry_price.toFixed(2)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      </CardContent>
    </Card>
  )
}

// pages/strategy/runModes.ts
// Desktop addition to the web's Detail page (SM-03, SM-05). An order is Live
// or Sandbox by the run that placed it. The server stamps every order row
// with its run's mode; the runs list, which holds at most 100 runs, only
// fills in for an order pushed over the socket. A run whose mode is not
// known is unknown, never Sandbox: the web read every run outside the 100 as
// Sandbox, so an older Live trade was counted as a Sandbox one.

import type { Order, Run, RunMode, Strategy } from '@/types/strategy_module'

/** An order row as the desktop server sends it: with its run's mode. */
export type ModedOrder = Order & { mode?: RunMode }

/**
 * The strategy as the desktop server sends it: with the realized P&L of every
 * finalised run summed per mode over the strategy's whole history.
 */
export type StrategyWithTotals = Strategy & {
  realized_by_mode?: Partial<Record<RunMode, number>>
}

export type RunModeIndex = ReadonlyMap<number, RunMode>

function isRunMode(value: unknown): value is RunMode {
  return value === 'live' || value === 'sandbox'
}

/** Run id to mode: the orders' own stamps first, then the runs list. */
export function runModeIndex(runs: Run[], orders: Order[]): RunModeIndex {
  const index = new Map<number, RunMode>()
  for (const run of runs) {
    if (isRunMode(run.mode)) index.set(run.id, run.mode)
  }
  for (const order of orders as ModedOrder[]) {
    if (isRunMode(order.mode)) index.set(order.run_id, order.mode)
  }
  return index
}

/** The orders placed by runs of `mode`. */
export function ordersOfMode(orders: Order[], index: RunModeIndex, mode: RunMode): Order[] {
  return orders.filter((order) => index.get(order.run_id) === mode)
}

export function modeLabel(mode: RunMode): string {
  return mode === 'live' ? 'Live' : 'Sandbox'
}

/** Realized P&L of every finalised run in `mode`; null when not known. */
export function finalisedRealized(strategy: Strategy, mode: RunMode | null): number | null {
  if (mode === null) return null
  const total = (strategy as StrategyWithTotals).realized_by_mode?.[mode]
  return typeof total === 'number' && Number.isFinite(total) ? total : null
}

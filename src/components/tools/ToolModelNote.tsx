import { cn } from '@/lib/utils'

/**
 * How a chain tool's numbers are computed, in one line under its charts:
 * the model, the price it is computed against and the part of the chain it
 * covers, so numbers that differ between tools (or from a whole-chain
 * figure elsewhere) are not read as errors.
 *
 * Desktop: the web pages do not say (TOOLS-15).
 */
export type ToolModel = 'gex' | 'ivsmile' | 'oi' | 'maxpain'

function scope(strikes: number): string {
  return `the ${strikes} strikes around the ATM strike shown here, not the whole option chain`
}

export function toolModelText(model: ToolModel, strikes: number): string {
  switch (model) {
    case 'gex':
      return (
        'GEX per strike is gamma × open interest × lot size, calls minus puts, with open ' +
        'interest as your broker reports it. Gamma uses the Black-76 model on the spot price; ' +
        "the Option Chain's Greeks use the synthetic future, so they can differ slightly. " +
        `Totals cover ${scope(strikes)}.`
      )
    case 'ivsmile':
      return (
        "Implied volatility uses the Black-76 model on the spot price; the Option Chain's " +
        'Greeks use the synthetic future, so their IVs can differ slightly. ' +
        `ATM IV and skew come from ${scope(strikes)}.`
      )
    case 'oi':
      return `Totals and PCR cover ${scope(strikes)}. Open interest is as your broker reports it.`
    case 'maxpain':
      return (
        `Max pain is worked out over ${scope(strikes)}, with open interest as your broker ` +
        'reports it.'
      )
  }
}

export function ToolModelNote({
  model,
  strikes,
  className,
}: {
  model: ToolModel
  strikes: number
  className?: string
}) {
  if (!(strikes > 0)) return null
  return (
    <p className={cn('text-xs text-muted-foreground', className)} data-testid="tool-model-note">
      {toolModelText(model, strikes)}
    </p>
  )
}

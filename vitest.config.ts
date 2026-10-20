import path from 'path'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

export default defineConfig({
  plugins: [react()],
  oxc: {
    jsx: {
      runtime: 'automatic',
      importSource: 'react',
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.{test,spec}.{js,mjs,cjs,ts,mts,cts,jsx,tsx}'],
    exclude: ['node_modules', 'dist', '.idea', '.git', '.cache'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json', 'json-summary', 'html'],
      // Desktop: every source file counts, also the ones no test imports.
      // Without this the denominator is only the files the tests happen to
      // load, which reads about 18 points higher than the truth.
      include: ['src/**/*.{ts,tsx}'],
      // Desktop: only what never runs in the app is left out: test helpers,
      // declaration files and config. Files under types/ stay in: some hold
      // executable helpers (strategy_module.ts, option-chain.ts), and the
      // rest are type-only, which adds nothing to either side of the ratio.
      exclude: ['node_modules/', 'src/test/', '**/*.d.ts', '**/*.config.*'],
      // Desktop: a ratchet, so CI fails when coverage drops. Raised on
      // 2026-10-20 from what CI measured on master (run 39249242727 at
      // ce5dd006: Node 20, 22 and 24 within 0.02 of each other on the
      // totals) and a local Node 24 run of the same tree, rounded down:
      // the totals and directories 1 point under the lower measurement,
      // the trading areas 2 points under it (V8's counts moved by up to
      // about a point between Node versions on these small groups in
      // October 2026). Raise these as coverage rises; never lower one, and
      // never exclude a file to meet it. The trading areas have floors of
      // their own, so a drop there cannot hide behind a rise elsewhere.
      thresholds: {
        lines: 61,
        statements: 60,
        functions: 55,
        branches: 53,
        'src/lib/**': { lines: 85, statements: 82, functions: 85, branches: 74 },
        'src/stores/**': { lines: 59, statements: 57, functions: 61, branches: 47 },
        'src/hooks/**': { lines: 75, statements: 74, functions: 76, branches: 62 },
        'src/types/**': { lines: 93, statements: 91, functions: 94, branches: 84 },
        // Orders: the book pages, order entry and the terminal's order dock.
        'src/pages/{OrderBook,TradeBook,Positions}.tsx': {
          lines: 51,
          statements: 49,
          functions: 46,
          branches: 41,
        },
        'src/components/trading/{PlaceOrderDialog,ExecuteBasketDialog,GttTab,dock/*}.{ts,tsx}': {
          lines: 55,
          statements: 53,
          functions: 45,
          branches: 49,
        },
        // Strategies: the strategy module and the strategy builder.
        'src/{pages/strategy/*,api/strategy_module,types/strategy_module,lib/trading/strategy*}.{ts,tsx}':
          { lines: 62, statements: 61, functions: 51, branches: 48 },
        'src/{pages/StrategyBuilder,components/strategy-builder/*,lib/strategyMath,lib/strategyTemplates}.{ts,tsx}':
          { lines: 87, statements: 84, functions: 81, branches: 77 },
        // Scalping and its protection (stop-loss, target, trailing).
        'src/{pages/Scalping,components/scalping/*,hooks/useTrailingSL,lib/scalping*,api/scalping}.{ts,tsx}':
          { lines: 51, statements: 48, functions: 42, branches: 43 },
        // Broker connection pages.
        'src/{pages/BrokerSelect,pages/BrokerTOTP,components/auth/Broker*,stores/brokerStore}.{ts,tsx}':
          { lines: 84, statements: 84, functions: 78, branches: 73 },
      },
    },
    css: true,
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
})

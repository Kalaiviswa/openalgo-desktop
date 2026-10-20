import AxeBuilder from '@axe-core/playwright'
import { expect, type Page, test } from '@playwright/test'

/**
 * Axe scans that fail on what they find.
 *
 * A violation of impact "serious" or "critical" fails the test unless its rule
 * is in ALLOWED below with the reason it was accepted. Lower impacts are
 * logged, not failed. An entry in ALLOWED is a reviewed decision, not a way to
 * make a red run green: name the pages it covers and why it cannot be fixed
 * now, and remove it once it is.
 */

type Impact = 'minor' | 'moderate' | 'serious' | 'critical'

const FAILING_IMPACTS: ReadonlySet<Impact> = new Set(['serious', 'critical'])

/** Rule id -> why it is accepted for now. */
const ALLOWED: Readonly<Record<string, string>> = {}

interface Finding {
  id: string
  impact: string | null | undefined
  targets: string[]
}

/** The serious and critical violations not accepted in ALLOWED. */
function blocking(
  violations: Awaited<ReturnType<AxeBuilder['analyze']>>['violations']
): Finding[] {
  return violations
    .filter((v) => FAILING_IMPACTS.has(v.impact as Impact) && !(v.id in ALLOWED))
    .map((v) => ({
      id: v.id,
      impact: v.impact,
      targets: v.nodes.map((n) => n.target.join(' ')),
    }))
}

async function scan(page: Page, configure: (axe: AxeBuilder) => AxeBuilder) {
  const results = await configure(new AxeBuilder({ page })).analyze()
  const logged = results.violations.filter((v) => !FAILING_IMPACTS.has(v.impact as Impact))
  if (logged.length > 0) {
    console.log(
      'Accessibility findings below the failing impact:',
      logged.map((v) => `${v.id} (${v.impact}): ${v.nodes.length} element(s)`)
    )
  }
  return blocking(results.violations)
}

const wcag = (axe: AxeBuilder) => axe.withTags(['wcag2a', 'wcag2aa'])

test.describe('Accessibility', () => {
  test('home page accessibility scan', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')

    expect(await scan(page, wcag)).toEqual([])
  })

  test('login page accessibility scan', async ({ page }) => {
    await page.goto('/login')
    await page.waitForLoadState('networkidle')

    expect(await scan(page, wcag)).toEqual([])
  })

  test('color contrast check', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')

    expect(await scan(page, (axe) => axe.withRules(['color-contrast']))).toEqual([])
  })

  // The check itself must be able to fail: a nameless button is a critical
  // "button-name" violation, so the same scan that passes the real page has
  // to report it once one is added.
  test('the scan reports a violation it is shown', async ({ page }) => {
    await page.goto('/login')
    await page.waitForLoadState('networkidle')
    await page.evaluate(() => {
      const button = document.createElement('button')
      button.id = 'axe-self-test'
      document.body.appendChild(button)
    })

    const found = await scan(page, wcag)
    expect(found).toContainEqual(
      expect.objectContaining({ id: 'button-name', targets: ['#axe-self-test'] })
    )
  })
})

/**
 * The signed-in pages a trader works in, with the signed-in shell (the
 * session is stubbed; every other call fails, so lists show their empty or
 * error states). Each must have no serious or critical finding beyond the
 * ones in KNOWN for it, and every KNOWN one must still be there: fixing one
 * means removing it here, and a new one fails.
 */
const SIGNED_IN_PAGES = [
  '/dashboard',
  '/orderbook',
  '/tradebook',
  '/positions',
  '/holdings',
  '/strategy',
  '/sandbox',
  '/logs',
  '/profile',
  '/tools',
] as const

/**
 * Findings on the signed-in pages still open, rule id per page. Empty: the
 * ones carried over from OpenAlgo web (recorded 2026-10-20) are fixed here,
 * and the same edits are listed for the web in docs/web-sync/a11y-2026-10-20.md.
 * A new finding fails; one accepted for a reason goes here with that reason.
 */
const KNOWN: Readonly<Partial<Record<(typeof SIGNED_IN_PAGES)[number], readonly string[]>>> = {}

async function signedIn(page: Page, theme: 'light' | 'dark' = 'light') {
  if (theme === 'dark') {
    // The theme store's persisted choice, as a trader who picked dark has it.
    await page.emulateMedia({ colorScheme: 'dark' })
    await page.addInitScript(() =>
      localStorage.setItem(
        'openalgo-theme',
        JSON.stringify({ state: { mode: 'dark', color: 'default', appMode: 'live' }, version: 0 })
      )
    )
  }
  await page.route('**/auth/session-status', (route) =>
    route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        status: 'success',
        logged_in: true,
        authenticated: true,
        broker: 'angel',
        username: 'e2e',
        active_sessions: 1,
      }),
    })
  )
}

test.describe('Accessibility - signed in', () => {
  for (const theme of ['light', 'dark'] as const) {
    for (const path of SIGNED_IN_PAGES) {
      test(`${path} accessibility scan${theme === 'dark' ? ' (dark)' : ''}`, async ({ page }) => {
        await signedIn(page, theme)
        await page.goto(path)
        await page.waitForLoadState('networkidle')
        await expect(page.getByTestId('navbar-row')).toBeVisible()

        const found = [...new Set((await scan(page, wcag)).map((f) => f.id))].sort()
        expect(found).toEqual([...(KNOWN[path] ?? [])].sort())
      })
    }
  }

  // States the stub does not reach by itself: each master contract status
  // the dashboard shows, and the password rules once some are met.
  for (const status of ['success', 'downloading', 'pending']) {
    test(`/dashboard with the master contract ${status}`, async ({ page }) => {
      await signedIn(page)
      await page.route('**/api/master-contract/status', (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ status, message: `Master contract ${status}` }),
        })
      )
      await page.goto('/dashboard')
      await page.waitForLoadState('networkidle')

      expect(await scan(page, wcag)).toEqual([])
    })
  }

  test('/profile with a new password typed', async ({ page }) => {
    await signedIn(page)
    await page.goto('/profile')
    await page.waitForLoadState('networkidle')
    await page.getByLabel('New Password', { exact: true }).fill('abcDEF12')

    expect(await scan(page, wcag)).toEqual([])
  })
})

test.describe('Accessibility - Mobile', () => {
  test.use({ viewport: { width: 375, height: 667 } })

  test('mobile layout accessibility scan', async ({ page }) => {
    await page.goto('/')
    await page.waitForLoadState('networkidle')

    expect(await scan(page, wcag)).toEqual([])
  })
})

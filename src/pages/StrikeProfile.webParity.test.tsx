import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'

/**
 * Strike Profile keeps the web page's behaviour as is (maintainer
 * 2026-10-20: Strike Profile matches the web exactly). It is therefore not
 * in the desktop's shared expiry-lookup and response-race suites
 * (`toolsExpiryDiscovery`, `toolsResponseRaces`); these cases pin what the
 * web page does instead: the nearest six expiries are selected and loaded,
 * a failed expiry lookup leaves the picker empty with the page's own
 * placeholder and no notice or Retry, and the expiry request is the web's.
 */

const mocks = vi.hoisted(() => ({
  strikeProfile: { getUnderlyings: vi.fn(), getExpiries: vi.fn(), getProfileData: vi.fn() },
}))

vi.mock('@/api/strike-profile', async (importActual) => {
  const actual = await importActual<typeof import('@/api/strike-profile')>()
  return { ...actual, strikeProfileApi: mocks.strikeProfile }
})
vi.mock('@/lib/Plot2D', () => ({ default: () => null }))
vi.mock('@/utils/toast', () => ({
  showToast: {
    error: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
    info: vi.fn(),
    dismissAll: vi.fn(),
    dynamic: vi.fn(),
    show: vi.fn(),
  },
}))
vi.mock('@/hooks/useSupportedExchanges', () => {
  const exchanges = [{ value: 'NFO', label: 'NFO' }]
  const value = {
    toolsFnoExchanges: exchanges,
    defaultToolsFnoExchange: 'NFO',
    fnoExchanges: exchanges,
    defaultFnoExchange: 'NFO',
    defaultUnderlyings: { NFO: ['NIFTY'] },
    isCrypto: false,
  }
  return { useSupportedExchanges: () => value }
})

const EXPIRIES = [
  '21-OCT-26',
  '28-OCT-26',
  '04-NOV-26',
  '11-NOV-26',
  '18-NOV-26',
  '25-NOV-26',
  '30-DEC-26',
]

async function mount() {
  const { default: Page } = await import('./StrikeProfile')
  render(
    <MemoryRouter>
      <Page />
    </MemoryRouter>
  )
}

beforeEach(() => {
  for (const fn of Object.values(mocks.strikeProfile)) fn.mockReset()
  mocks.strikeProfile.getUnderlyings.mockResolvedValue({
    status: 'success',
    underlyings: ['NIFTY'],
  })
  mocks.strikeProfile.getProfileData.mockImplementation(() => new Promise(() => {}))
})

describe('Strike Profile, as the web page', () => {
  it('selects the nearest six expiries and loads them at a 5% window', async () => {
    mocks.strikeProfile.getExpiries.mockResolvedValue({ status: 'success', expiries: EXPIRIES })
    await mount()
    await waitFor(() => expect(mocks.strikeProfile.getProfileData).toHaveBeenCalled())
    expect(mocks.strikeProfile.getProfileData).toHaveBeenLastCalledWith({
      underlying: 'NIFTY',
      exchange: 'NFO',
      expiry_dates: ['21OCT26', '28OCT26', '04NOV26', '11NOV26', '18NOV26', '25NOV26'],
      range_pct: 5,
    })
    expect(screen.getByText('6 of 7 expiries')).toBeInTheDocument()
  })

  it('leaves a failed expiry lookup as an empty picker, with no notice or Retry', async () => {
    mocks.strikeProfile.getExpiries.mockRejectedValue(new Error('Network Error'))
    await mount()
    await waitFor(() =>
      expect(mocks.strikeProfile.getExpiries).toHaveBeenCalledWith('NFO', 'NIFTY')
    )
    expect(
      await screen.findByText('Select an underlying and expiries to view the strike profile')
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /Expiries/ })).toBeDisabled()
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull()
    expect(mocks.strikeProfile.getProfileData).not.toHaveBeenCalled()
  })

  it('sends the web expiry request as is, the underlying unencoded', async () => {
    const actual =
      await vi.importActual<typeof import('@/api/strike-profile')>('@/api/strike-profile')
    const { webClient } = await import('@/api/client')
    const seen: string[] = []
    const original = webClient.defaults.adapter
    webClient.defaults.adapter = async (config) => {
      seen.push(config.url ?? '')
      return {
        data: { status: 'success', expiries: [] },
        status: 200,
        statusText: '',
        headers: {},
        config,
      }
    }
    try {
      await actual.strikeProfileApi.getExpiries('NFO', 'M&M')
    } finally {
      webClient.defaults.adapter = original
    }
    expect(seen).toEqual([
      '/search/api/expiries?exchange=NFO&underlying=M&M&instrumenttype=options',
    ])
  })
})

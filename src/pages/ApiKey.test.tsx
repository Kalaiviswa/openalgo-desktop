import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { MemoryRouter } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useAuthStore } from '@/stores/authStore'
import ApiKey from './ApiKey'

// The AI client tokens card asks the server for its own list; not under test.
vi.mock('@/components/desktop/McpTokens', () => ({ McpTokens: () => null }))

const OLD_KEY = 'fake-apikey-old-0123456789abcdef'
const NEW_KEY = 'fake-apikey-new-0123456789abcdef'

function json(body: unknown) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  )
}

describe('ApiKey page', () => {
  beforeEach(() => {
    useAuthStore.setState({
      user: { username: 'trader', broker: 'zerodha', isLoggedIn: true, loginTime: null },
      isAuthenticated: true,
      apiKey: OLD_KEY,
    })
    vi.stubGlobal(
      'fetch',
      vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input)
        if (url === '/auth/csrf-token') return json({ csrf_token: 'csrf' })
        if (url === '/apikey' && init?.method === 'POST') return json({ api_key: NEW_KEY })
        if (url === '/apikey') return json({ api_key: OLD_KEY, order_mode: 'auto' })
        return json({})
      })
    )
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  // The key is kept in memory for the books, quotes and order pages; the old
  // one stops working on regeneration, so they must move to the new one.
  it('puts a regenerated key in memory for the other pages', async () => {
    const user = userEvent.setup()
    render(
      <MemoryRouter>
        <ApiKey />
      </MemoryRouter>
    )

    await user.click(await screen.findByRole('button', { name: /^regenerate$/i }))
    const confirm = await screen.findByRole('dialog', { name: 'Regenerate API Key?' })
    await user.click(within(confirm).getByRole('button', { name: /^regenerate$/i }))

    await waitFor(() => expect(useAuthStore.getState().apiKey).toBe(NEW_KEY))
  })
})

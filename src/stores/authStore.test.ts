import { beforeEach, describe, expect, it, vi } from 'vitest'

// The store's own localStorage entry. The WebView keeps localStorage in a
// plain file on disk, so the API key must never be written there.
const STORAGE_KEY = 'openalgo-auth'
const FAKE_KEY = 'fake-apikey-0123456789abcdef'

function stored(): { state: Record<string, unknown>; version: number } {
  return JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null')
}

/** A fresh copy of the store, as the window loads it at start. */
async function loadStore() {
  vi.resetModules()
  return (await import('./authStore')).useAuthStore
}

describe('auth store persistence', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('keeps the API key in memory and out of storage', async () => {
    const useAuthStore = await loadStore()
    useAuthStore.getState().login('trader', 'zerodha')
    useAuthStore.getState().setApiKey(FAKE_KEY)

    expect(useAuthStore.getState().apiKey).toBe(FAKE_KEY)
    const saved = stored()
    expect(saved.version).toBe(1)
    expect(Object.keys(saved.state).sort()).toEqual(['isAuthenticated', 'user'])
    expect(saved.state.isAuthenticated).toBe(true)
    expect(saved.state.user).toMatchObject({ username: 'trader', broker: 'zerodha' })
    expect(localStorage.getItem(STORAGE_KEY)).not.toContain(FAKE_KEY)
  })

  it('erases the key a 1.0.0 install stored, as soon as the store loads', async () => {
    const user = {
      username: 'trader',
      broker: 'zerodha',
      isLoggedIn: true,
      loginTime: '2026-10-20T04:00:00.000Z',
    }
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ state: { user, apiKey: FAKE_KEY, isAuthenticated: true }, version: 0 })
    )

    const useAuthStore = await loadStore()

    // Written back on load, before any sign-in or other change.
    expect(localStorage.getItem(STORAGE_KEY)).not.toContain(FAKE_KEY)
    expect(stored()).toEqual({ state: { user, isAuthenticated: true }, version: 1 })
    // Not read into memory either: AuthSync asks the server for the key.
    expect(useAuthStore.getState().apiKey).toBeNull()
    expect(useAuthStore.getState().user).toEqual(user)
    expect(useAuthStore.getState().isAuthenticated).toBe(true)
  })

  it('loads a stored 1.0.0 entry without a user as signed out', async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ state: { apiKey: FAKE_KEY }, version: 0 }))

    const useAuthStore = await loadStore()

    expect(stored()).toEqual({ state: { user: null, isAuthenticated: false }, version: 1 })
    expect(useAuthStore.getState().isAuthenticated).toBe(false)
  })

  it('forgets the key and the key check at logout', async () => {
    const useAuthStore = await loadStore()
    useAuthStore.getState().login('trader', 'zerodha')
    useAuthStore.getState().setApiKey(FAKE_KEY)
    useAuthStore.setState({ apiKeyChecked: true })

    useAuthStore.getState().logout()

    expect(useAuthStore.getState()).toMatchObject({
      apiKey: null,
      apiKeyChecked: false,
      isAuthenticated: false,
      user: null,
    })
    expect(stored().state).toEqual({ user: null, isAuthenticated: false })
  })
})

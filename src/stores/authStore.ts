import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { useBrokerStore } from './brokerStore'

interface User {
  username: string
  broker: string | null
  isLoggedIn: boolean
  loginTime: string | null
}

interface AuthStore {
  user: User | null
  /** Desktop: in memory only, never in storage (see `persist` below). */
  apiKey: string | null
  isAuthenticated: boolean
  /**
   * Desktop: whether AuthSync has asked the server for the API key since the
   * last logout. Until it has, AuthSync holds back the pages that read the
   * key, so none of them starts without a key the trader has. In memory only.
   */
  apiKeyChecked: boolean

  setUser: (user: User) => void
  setApiKey: (apiKey: string | null) => void
  login: (username: string, broker: string) => void
  logout: () => void
  checkSession: () => boolean
}

/**
 * Desktop: the only fields kept in browser storage, and the only ones read
 * back from it. Whatever else an entry holds (the API key 1.0.0 wrote, or an
 * entry at the current version that should not exist) never reaches memory:
 * the key comes from the server once it has checked the session (AuthSync).
 */
function storedFields(stored: unknown): Pick<AuthStore, 'user' | 'isAuthenticated'> {
  const { user = null, isAuthenticated = false } = (stored ?? {}) as Partial<AuthStore>
  return { user, isAuthenticated }
}

export const useAuthStore = create<AuthStore>()(
  persist(
    (set, get) => ({
      user: null,
      apiKey: null,
      isAuthenticated: false,
      apiKeyChecked: false,

      setUser: (user) => set({ user, isAuthenticated: user.isLoggedIn }),

      setApiKey: (apiKey) => set({ apiKey }),

      login: (username, broker) => {
        const user: User = {
          username,
          broker,
          isLoggedIn: true,
          loginTime: new Date().toISOString(),
        }
        set({ user, isAuthenticated: true })
      },

      logout: () => {
        set({ user: null, isAuthenticated: false, apiKey: null, apiKeyChecked: false })
        // The broker's capabilities belong to the session that just ended.
        useBrokerStore.getState().clearCapabilities()
      },

      checkSession: () => {
        const { user } = get()
        if (!user || !user.loginTime) return false

        // Skip session expiry for crypto brokers (24/7 markets)
        const capabilities = useBrokerStore.getState().capabilities
        if (capabilities?.broker_type === 'crypto') {
          return true
        }

        // Session expiry check (3 AM IST daily)
        const now = new Date()
        const loginTime = new Date(user.loginTime)

        // Convert to IST properly: UTC + 5.5 hours
        // First get UTC time, then add IST offset
        const istOffsetMs = 5.5 * 60 * 60 * 1000
        const localOffsetMs = now.getTimezoneOffset() * 60 * 1000

        // Convert current time to IST
        const nowUTC = now.getTime() + localOffsetMs
        const nowIST = new Date(nowUTC + istOffsetMs)

        // Convert login time to IST
        const loginUTC = loginTime.getTime() + localOffsetMs
        const loginIST = new Date(loginUTC + istOffsetMs)

        // Create today's 3 AM IST expiry time
        const todayExpiry = new Date(nowIST)
        todayExpiry.setHours(3, 0, 0, 0)

        // If current time is after 3 AM IST today and login was before 3 AM IST today
        if (nowIST > todayExpiry && loginIST < todayExpiry) {
          get().logout()
          return false
        }

        return true
      },
    }),
    {
      name: 'openalgo-auth',
      // Desktop: the API key is never written here. This storage is a plain
      // file in the WebView profile, while every other secret is encrypted at
      // rest with the login password (KEY-01), so the key stays in memory and
      // AuthSync asks the server for it after each start.
      partialize: (state) => storedFields(state),
      version: 1,
      // Version 0 (1.0.0) stored the key. A migration makes zustand write the
      // store back at once, without it.
      migrate: storedFields,
      // zustand's own merge copies every stored field when the version
      // matches, so it alone would read a stored key into memory.
      merge: (stored, current) => ({ ...current, ...storedFields(stored) }),
    }
  )
)

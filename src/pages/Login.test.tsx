import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@/test/test-utils'
import Login from './Login'

// Desktop: one sign-in card. After the password the server says where to
// go: the dashboard when the saved broker session was resumed, otherwise
// Select Broker. No page in between.

function serve(opts: { needsSetup?: boolean; login: Record<string, unknown>; totp?: unknown }) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })
      if (url === '/auth/check-setup') return json({ needs_setup: opts.needsSetup ?? false })
      if (url === '/auth/session-status') {
        return json({ status: 'success', authenticated: false, logged_in: false })
      }
      if (url === '/auth/csrf-token') return json({ csrf_token: 'tok' })
      if (url === '/auth/login') {
        return new Response(JSON.stringify(opts.login), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        })
      }
      if (url === '/auth/login/totp') return json(opts.totp ?? { status: 'success' })
      return json({}, 404)
    })
  )
}

async function signIn() {
  fireEvent.change(await screen.findByLabelText('Username'), { target: { value: 'trader' } })
  fireEvent.change(screen.getByLabelText('Password'), { target: { value: 'Secret@123' } })
  fireEvent.click(screen.getByRole('button', { name: /sign in/i }))
}

beforeEach(() => {
  window.history.pushState({}, '', '/login')
})

afterEach(() => {
  vi.unstubAllGlobals()
  window.history.pushState({}, '', '/')
})

describe('Sign-in routing', () => {
  it('goes straight to the dashboard when the broker session was resumed', async () => {
    serve({
      login: { status: 'success', redirect: '/dashboard', broker: 'zerodha' },
    })
    render(<Login />)
    await signIn()
    await waitFor(() => expect(window.location.pathname).toBe('/dashboard'))
  })

  it('goes to Select Broker when no broker session was resumed', async () => {
    serve({ login: { status: 'success', redirect: '/broker' } })
    render(<Login />)
    await signIn()
    await waitFor(() => expect(window.location.pathname).toBe('/broker'))
  })

  it('goes to Select Broker when the server names no page', async () => {
    serve({ login: { status: 'success' } })
    render(<Login />)
    await signIn()
    await waitFor(() => expect(window.location.pathname).toBe('/broker'))
  })

  it('asks for the authenticator code first when two-factor sign-in is on', async () => {
    serve({
      login: { status: 'totp_required', message: 'Enter the 6-digit code.' },
      totp: { status: 'success', redirect: '/broker' },
    })
    render(<Login />)
    await signIn()
    const code = await screen.findByLabelText('Authentication code')
    expect(window.location.pathname).toBe('/login')
    fireEvent.change(code, { target: { value: '123456' } })
    fireEvent.click(screen.getByRole('button', { name: 'Verify code' }))
    await waitFor(() => expect(window.location.pathname).toBe('/broker'))
  })

  it("shows the server's reason and stays on the sign-in card", async () => {
    serve({ login: { status: 'error', message: 'Invalid credentials' } })
    render(<Login />)
    await signIn()
    expect(await screen.findByText('Invalid credentials')).toBeInTheDocument()
    expect(window.location.pathname).toBe('/login')
  })
})

describe('Sign-in card', () => {
  it('offers account setup only when no account exists yet', async () => {
    serve({ needsSetup: true, login: { status: 'success' } })
    const { unmount } = render(<Login />)
    expect(await screen.findByRole('link', { name: 'Set up your account' })).toHaveAttribute(
      'href',
      '/setup'
    )
    unmount()
    vi.unstubAllGlobals()
    serve({ needsSetup: false, login: { status: 'success' } })
    render(<Login />)
    await screen.findByLabelText('Username')
    expect(screen.queryByRole('link', { name: 'Set up your account' })).toBeNull()
    // The recovery path (password reset, then the reset-account card).
    expect(screen.getByRole('link', { name: 'Forgot password?' })).toHaveAttribute(
      'href',
      '/reset-password'
    )
  })
})

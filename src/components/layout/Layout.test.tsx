import { render, screen } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router'
import { afterEach, describe, expect, it } from 'vitest'
import { useAuthStore } from '@/stores/authStore'
import { FullWidthLayout } from './FullWidthLayout'
import { Layout } from './Layout'

// Desktop: a signed-in trader with no broker logged in lands on Select
// Broker directly; one who is not signed in lands on the sign-in page.

function at(path: string, full = false) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route element={full ? <FullWidthLayout /> : <Layout />}>
          <Route path="/dashboard" element={<div>dashboard</div>} />
          <Route path="/trading" element={<div>trading</div>} />
        </Route>
        <Route path="/login" element={<div>sign-in page</div>} />
        <Route path="/broker" element={<div>select broker page</div>} />
      </Routes>
    </MemoryRouter>
  )
}

afterEach(() => {
  useAuthStore.setState({ user: null, isAuthenticated: false, apiKey: null })
})

describe('app layout guards', () => {
  it.each([
    ['/dashboard', false],
    ['/trading', true],
  ])('send %s to sign-in when not signed in', (path, full) => {
    useAuthStore.setState({ user: null, isAuthenticated: false })
    at(path, full)
    expect(screen.getByText('sign-in page')).toBeInTheDocument()
  })

  it.each([
    ['/dashboard', false],
    ['/trading', true],
  ])('send %s straight to Select Broker when no broker is logged in', (path, full) => {
    // As AuthSync stores a signed-in session without a broker.
    useAuthStore.setState({
      user: { username: 'trader', broker: null, isLoggedIn: false, loginTime: null },
      isAuthenticated: false,
    })
    at(path, full)
    expect(screen.getByText('select broker page')).toBeInTheDocument()
  })
})

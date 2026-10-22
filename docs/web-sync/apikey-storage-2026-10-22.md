# The API key in browser storage, a fix to carry to OpenAlgo web (2026-10-22)

OpenAlgo web's auth store writes the trader's OpenAlgo API key, in plain
text, to the browser's `localStorage`. OpenAlgo Desktop carried the store over
unchanged, so 1.0.0 wrote the key to the WebView profile on disk, beside a
database in which every other secret is encrypted with the login password
(KEY-01). The maintainer decided (2026-10-22) to keep the key in memory only
and fetch it when a page needs it, in the desktop's 1.0.1, and to note the
same change for the web.

Web line numbers are from the web checkout of 2026-10-18; the web's
`authStore.ts` has not changed upstream since 2026-03-18 (79962e267). Search
for the old text if the lines have moved.

## The defect on the web

`frontend/src/stores/authStore.ts` wraps the store in zustand's `persist`
with only a name (line 90, `name: 'openalgo-auth'`). Without `partialize`,
`persist` writes the whole state, `apiKey` included (line 14), on every
change. In the browser, that means:

- the key sits in the browser profile of every machine that opened the
  dashboard, readable by anyone with access to that profile, and by any
  script running on the OpenAlgo origin;
- `logout()` (line 46) clears it, but only when it runs: a browser closed
  while signed in leaves the key there until the dashboard is opened again;
- nothing needs the stored copy. `AuthSync` reads the key from
  `/auth/session-status` on every page load (line 42), before any page
  renders.

One thing does lean on the store's copy. A broker that signs in on its own
card (`frontend/src/pages/BrokerTOTP.tsx` line 406, `login()` then
`navigate('/dashboard')`) does not reload the page, and the web's `AuthSync`
checks the session only when the page loads, so nothing fetches the key for
that session: the pages use whatever key the store already holds. Without a
stored key, the books, quotes and order forms would have none until the page
is reloaded, so the `AuthSync` change below is part of the fix, not an extra.
The desktop met exactly this (commit 178d1902): every launch starts signed
out, so the check at load empties the store before the trader signs in.

## What the desktop changed

| Desktop file | Change |
| --- | --- |
| `src/stores/authStore.ts` | `persist` keeps `user` and `isAuthenticated` only (`partialize`), at version 1. A `migrate` from version 0 drops `apiKey`, which also makes zustand write the stored entry back as soon as the store loads, so an existing install loses its stored key at the first start. New in-memory flag `apiKeyChecked`, cleared by `logout()`. |
| `src/components/auth/AuthSync.tsx` | Checks the session again on each page change while signed in without a key (desktop commit 178d1902, before this change). Now also holds back a page that needs a broker until that check has answered (`apiKeyChecked`), so no page starts without the key: an empty book or "generate an API key" notice that fixes itself a moment later. A logout starts the wait again. |
| `src/pages/ApiKey.tsx` | A regenerated key also goes into the store. Before, every page that reads the store kept sending the old key, which the server had just invalidated, until the page was reloaded. The web has the same stale key. |

Tests: `src/stores/authStore.test.ts` (nothing but `user` and
`isAuthenticated` is written; a version-0 entry holding a key is rewritten
without it on load and the key is not read into memory),
`src/components/auth/AuthSync.test.tsx` (a page waits for the key after a
sign-in and first renders with it; it goes ahead when the trader has no key or
the server cannot be reached; it waits again after a logout),
`src/pages/ApiKey.test.tsx` (a regenerated key reaches the store).

Pages that fetch the key themselves were not changed and do not read the
store: `ApiKey`, `Playground`, `TradingView`, `GoCharting`, `WebSocketTest`,
`WebSocketOrder`, `HistorifyCharts`, the `/trading` terminal and
`MarketDataManager` (`/api/websocket/apikey`).

## Suggested web change

### `frontend/src/stores/authStore.ts`

At line 90, replace `name: 'openalgo-auth',` with:

```ts
      name: 'openalgo-auth',
      // The API key stays in memory; AuthSync fetches it on each page load.
      partialize: (state) => ({ user: state.user, isAuthenticated: state.isAuthenticated }),
      version: 1,
      // Version 0 stored the key. A migration makes zustand rewrite the
      // stored entry at once, without it.
      migrate: (stored) => {
        const { user = null, isAuthenticated = false } = (stored ?? {}) as Partial<AuthStore>
        return { user, isAuthenticated }
      },
```

`partialize` alone is not enough for browsers that already hold a key:
zustand writes storage after hydration only when the state changes or a
migration ran, so the version bump and `migrate` are what erase the existing
copy on the first load.

Add the in-memory flag `apiKeyChecked: boolean` to the `AuthStore` interface
(after line 15), initialise it to `false` (after line 29) and clear it in
`logout()` (line 46: `set({ user: null, isAuthenticated: false, apiKey: null,
apiKeyChecked: false })`). `partialize` leaves it out of storage.

### `frontend/src/components/auth/AuthSync.tsx`

Take the desktop's `AuthSync` (`src/components/auth/AuthSync.tsx`) as the
model; it differs from the web's only by these additions:

1. `beforeBroker(pathname)`: the public pages and the broker sign-in pages,
   where a session without a broker is expected.
2. The session check becomes a `useCallback` guarded by a `useRef`, and a
   second effect runs it again on each page change while
   `isAuthenticated && !apiKey && !beforeBroker(pathname)`. This closes the
   TOTP broker gap above.
3. In the check's `finally` (line 74), when the trader is still signed in,
   set `apiKeyChecked: true`.
4. Before the spinner at line 83, compute
   `awaitingKey = isAuthenticated && !apiKey && !apiKeyChecked && !beforeBroker(pathname)`
   and show the spinner while `isChecking || awaitingKey`.

### `frontend/src/pages/ApiKey.tsx`

At line 119, after `setApiKey(data.api_key)` (the page's own state), add
`useAuthStore.getState().setApiKey(data.api_key)`.

## Not changed, and nothing to carry

No other frontend code writes the key to `localStorage`, `sessionStorage`
or IndexedDB, in the desktop or in the web (every file using browser storage
was checked, on both sides). Playground keeps no request history in browser
storage; TanStack Query is not persisted; the `/trading` terminal's saved
workspaces and per-pane preferences leave the key out (the desktop's
`workspaceState.test.ts` checks this); the error reporter redacts `apikey`
from the URLs it sends.

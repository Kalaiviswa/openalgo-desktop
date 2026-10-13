import { useEffect, useMemo, useRef } from 'react'

/**
 * Which of a page's requests is the current one.
 *
 * `begin()` starts a request and returns its token; only a token that is
 * still `isCurrent` may change the page (its data, its error, its loading
 * state). `invalidate()` drops every request in flight, for when what the
 * page shows changes identity before the next request starts. Unmounting
 * invalidates too.
 *
 * Desktop: the web's chart pages accept whichever answer lands last, so a
 * slow answer for a previous selection overwrites the current one
 * (TOOLS-07).
 */
export function useRequestGeneration() {
  const generation = useRef(0)
  useEffect(
    () => () => {
      generation.current += 1
    },
    []
  )
  return useMemo(
    () => ({
      begin: () => {
        generation.current += 1
        return generation.current
      },
      isCurrent: (token: number) => token === generation.current,
      invalidate: () => {
        generation.current += 1
      },
    }),
    []
  )
}

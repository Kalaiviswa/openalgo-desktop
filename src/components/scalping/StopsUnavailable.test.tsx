import { render, screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { STOPS_UNAVAILABLE_TEXT, StopsUnavailable } from './StopsUnavailable'

describe('StopsUnavailable (NEW-TR-07)', () => {
  it('says the stops shown may be out of date when the list could not be read', () => {
    render(<StopsUnavailable show />)
    expect(screen.getByRole('alert')).toHaveTextContent(STOPS_UNAVAILABLE_TEXT)
  })

  it('says nothing while the list reads', () => {
    const { container } = render(<StopsUnavailable show={false} />)
    expect(container).toBeEmptyDOMElement()
  })
})

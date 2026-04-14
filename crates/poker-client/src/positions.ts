/**
 * Poker table positions for 2-10 players.
 *
 * Position names derive from the button position:
 *   BTN (dealer) → SB → BB → UTG → UTG+1 → MP → HJ → CO → BTN
 *
 * In heads-up (2 players): BTN/SB and BB.
 */

const POSITIONS: Record<number, string[]> = {
  2: ['BTN/SB', 'BB'],
  3: ['BTN', 'SB', 'BB'],
  4: ['BTN', 'SB', 'BB', 'CO'],
  5: ['BTN', 'SB', 'BB', 'UTG', 'CO'],
  6: ['BTN', 'SB', 'BB', 'UTG', 'HJ', 'CO'],
  7: ['BTN', 'SB', 'BB', 'UTG', 'MP', 'HJ', 'CO'],
  8: ['BTN', 'SB', 'BB', 'UTG', 'UTG+1', 'MP', 'HJ', 'CO'],
  9: ['BTN', 'SB', 'BB', 'UTG', 'UTG+1', 'MP', 'HJ', 'LJ', 'CO'],
  10: ['BTN', 'SB', 'BB', 'UTG', 'UTG+1', 'UTG+2', 'MP', 'HJ', 'LJ', 'CO'],
}

/**
 * Get position label for a seat at the table.
 * @param seat - seat index (0-9)
 * @param button - button seat index
 * @param numPlayers - total players at table
 */
export function getPosition(seat: number, button: number, numPlayers: number): string {
  const positions = POSITIONS[numPlayers] || POSITIONS[10]!
  const offset = (seat - button + numPlayers) % numPlayers
  return positions[offset] || `S${seat}`
}

/**
 * Get short position (2-3 chars) for compact display.
 */
export function getPositionShort(seat: number, button: number, numPlayers: number): string {
  const pos = getPosition(seat, button, numPlayers)
  if (pos === 'BTN/SB') return 'SB'
  if (pos.startsWith('UTG+')) return 'U' + pos.slice(4)
  return pos.slice(0, 3)
}

/**
 * Seat layout angles for N players around an elliptical table.
 * Returns [x%, y%] positions for each seat (0-based from hero at bottom).
 */
export function getSeatPositions(numPlayers: number, heroSeat: number): Array<{ x: number; y: number; isTop: boolean }> {
  const positions: Array<{ x: number; y: number; isTop: boolean }> = []
  for (let i = 0; i < numPlayers; i++) {
    // angle: hero at bottom (180°), others distributed clockwise
    const offset = (i - heroSeat + numPlayers) % numPlayers
    const angle = Math.PI + (offset / numPlayers) * 2 * Math.PI
    // elliptical layout: wider than tall
    const x = 50 + 40 * Math.sin(angle)
    const y = 50 - 35 * Math.cos(angle)
    const isTop = y < 50
    positions.push({ x, y, isTop })
  }
  return positions
}

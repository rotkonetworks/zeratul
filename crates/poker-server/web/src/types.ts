export type CardJson = { rank: string; suit: string }
export type ValidAction = { kind: string; min_amount: number; max_amount: number }
export type PotJson = { amount: number; eligible: number[] }

export type ServerMsg =
  | { type: 'Seated'; seat: number; name: string }
  | { type: 'Waiting' }
  | { type: 'OpponentJoined'; seat: number; name: string }
  | { type: 'OpponentLeft'; seat: number }
  | { type: 'HandStarted'; hand_number: number; button: number; your_cards: [CardJson, CardJson] | null; stacks: number[] }
  | { type: 'BlindsPosted'; small_blind: [number, number]; big_blind: [number, number] }
  | { type: 'ActionRequired'; seat: number; valid_actions: ValidAction[] }
  | { type: 'PlayerActed'; seat: number; action: string; amount: number; new_stack: number }
  | { type: 'CommunityCards'; phase: string; cards: CardJson[] }
  | { type: 'PotUpdate'; pots: PotJson[] }
  | { type: 'Showdown'; hands: [number, [CardJson, CardJson]][] }
  | { type: 'PotAwarded'; seat: number; amount: number }
  | { type: 'HandComplete'; stacks: number[] }
  | { type: 'Error'; message: string }

export type CardJson = { rank: string; suit: string }
export type ValidAction = { kind: string; min_amount: number; max_amount: number }
export type PotJson = { amount: number; eligible: number[] }

export type ServerMsg =
  | { type: 'Seated'; seat: number; name: string }
  | { type: 'Waiting' }
  | { type: 'OpponentJoined'; seat: number; name: string }
  | { type: 'OpponentLeft'; seat: number }
  | { type: 'OpponentDisconnected'; seat: number; reconnect_secs: number }
  | { type: 'OpponentReconnected'; seat: number }
  | { type: 'ActionTimeout'; seat: number }
  | { type: 'TimerTick'; secondsLeft: number }
  | { type: 'HandStarted'; hand_number: number; button: number; your_cards: [CardJson, CardJson] | null; stacks: number[] }
  | { type: 'BlindsPosted'; small_blind: [number, number]; big_blind: [number, number] }
  | { type: 'ActionRequired'; seat: number; valid_actions: ValidAction[] }
  | { type: 'PlayerActed'; seat: number; action: string; amount: number; new_stack: number }
  | { type: 'CommunityCards'; phase: string; cards: CardJson[] }
  | { type: 'PotUpdate'; pots: PotJson[] }
  | { type: 'Showdown'; hands: [number, [CardJson, CardJson]][] }
  | { type: 'PotAwarded'; seat: number; amount: number }
  | { type: 'HandComplete'; stacks: number[] }
  | { type: 'JuryVote'; node: number; total: number; payload_hash: string }
  | { type: 'JurySettlement'; verified: boolean; threshold: number; contributions: number }
  | { type: 'RulesProposed'; buyin: number; smallBlind: number; bigBlind: number; fromSelf: boolean }
  | { type: 'RulesAccepted' }
  | { type: 'RoomInfo'; code: string; jury_nodes: number; jury_threshold: number; escrow: string }
  | { type: 'InviteLink'; url: string }
  | { type: 'Status'; phase: 'connecting' | 'encrypting' | 'shuffling' | 'dealing' | 'playing' | 'showdown' | 'settling'; message: string }
  | { type: 'Error'; message: string }

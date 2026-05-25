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
  | { type: 'ActionPaused'; seat: number }
  | { type: 'ActionResumed'; seat: number; seconds_left: number }
  | { type: 'ActionTimeout'; seat: number }
  | { type: 'TimerTick'; seat: number; seconds_left: number }
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
  | { type: 'RoomInfo'; code: string; jury_nodes: number; jury_threshold: number; escrow: string; frost_relay_url?: string; frost_room_code?: string; seat_addresses?: (string | null)[]; required_deposit: number; buyin_zat?: number; fee_per_seat?: number }
  | { type: 'GameOver'; reason: string; payouts: [number, number][] }
  | { type: 'PayoutSigningRequest'; relay_room: string; plan: { seat: number; address: string; amount_zat: number }[]; priority_seat: number }
  | { type: 'PayoutComplete'; txid: string }
  | { type: 'PayoutFailed'; reason: string }
  | { type: 'OpponentAbandoned'; seat: number }
  | { type: 'DepositStatus'; escrow_address: string; seat_addresses?: (string | null)[]; player_a_deposit: number; player_b_deposit: number; required: number; ready: boolean }
  | { type: 'InviteLink'; url: string }
  | { type: 'Status'; phase: 'connecting' | 'encrypting' | 'shuffling' | 'dealing' | 'playing' | 'showdown' | 'settling'; message: string }
  | { type: 'Chat'; from: string; text: string }
  | { type: 'Error'; message: string }

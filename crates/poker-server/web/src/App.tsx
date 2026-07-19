import { createSignal, For, Show, createEffect, onCleanup } from 'solid-js'
import { createSocket } from './ws'
import { Card } from './Card'
import Lobby, { type Table } from './Lobby'
import { Settings } from './Settings'
import Tournaments, { reportTournamentResult } from './Tournaments'
import { detectZafu } from './zid/provider'
import { getPositionShort } from './positions'
import { requestPokerDkg, requestDeletePokerMultisig, requestPokerSign } from './dkg'
import type { ServerMsg, CardJson, ValidAction } from './types'

export default function App() {
  const [view, setView] = createSignal<'casino' | 'lobby' | 'waiting' | 'deposit' | 'game' | 'settlement'>(
    // a tournament deep-link (/t, /t/<id>) shows the hub underneath the overlay — NOT a table
    // lobby for a room called "t". A bare /<code> still lands in the table lobby.
    (() => { const p = location.pathname.replace(/^\/+|\/+$/g, ''); return (!p || p === 't' || p.startsWith('t/')) ? 'casino' : 'lobby' })()
  )
  // settlement state: starts at 'preparing' on GameOver while escrow builds the PCZT,
  // transitions on PayoutSigningRequest → 'pending' → user signs → 'complete' / 'failed'.
  type SettlementStatus =
    | { phase: 'preparing' }
    | { phase: 'pending' }
    | { phase: 'signing' }
    | { phase: 'complete'; txid: string }
    | { phase: 'failed'; reason: string }
  const [settleStatus, setSettleStatus] = createSignal<SettlementStatus>({ phase: 'pending' })
  const [settleRelayRoom, setSettleRelayRoom] = createSignal('')
  const [settlePlan, setSettlePlan] = createSignal<{ seat: number; address: string; amount_zat: number }[]>([])
  const [settlePrioritySeat, setSettlePrioritySeat] = createSignal(-1)
  const [settleReason, setSettleReason] = createSignal('')
  const [settleFrostRelay, setSettleFrostRelay] = createSignal('')
  // tick for the fallback-signer countdown. Server swaps priority every 90s of inactivity;
  // the SPA tracks Date.now() at each PayoutSigningRequest and renders 90 - elapsed.
  const SETTLE_FALLBACK_SECS = 90
  const [settleBroadcastAt, setSettleBroadcastAt] = createSignal(0)
  const [settleFallbackTick, setSettleFallbackTick] = createSignal(SETTLE_FALLBACK_SECS)
  createEffect(() => {
    const baseAt = settleBroadcastAt()
    const phase = settleStatus().phase
    if (!baseAt || (phase !== 'pending' && phase !== 'signing')) return
    const tick = () => {
      const left = Math.max(0, SETTLE_FALLBACK_SECS - Math.floor((Date.now() - baseAt) / 1000))
      setSettleFallbackTick(left)
    }
    tick()
    const iv = setInterval(tick, 1000)
    onCleanup(() => clearInterval(iv))
  })
  // deposit-panel state — populated by RoomInfo + DepositStatus
  const [requiredDeposit, setRequiredDeposit] = createSignal(0)
  const [depositBuyinZat, setDepositBuyinZat] = createSignal(0)
  const [depositFeePerSeat, setDepositFeePerSeat] = createSignal(0)
  const [seatAddresses, setSeatAddresses] = createSignal<(string | null)[]>([])
  const [depositA, setDepositA] = createSignal(0)
  const [depositB, setDepositB] = createSignal(0)
  // mempool-seen (0-conf) per-seat value — UX only, never money. Lets the deposit view show
  // "seen in mempool — dealing…" the instant a tx lands, instead of waiting ~75s for a block.
  const [pendingA, setPendingA] = createSignal(0)
  const [pendingB, setPendingB] = createSignal(0)
  const [depositReady, setDepositReady] = createSignal(false)
  // set true once the player has triggered a Send-with-zafu — used to warn against double-sending
  // while the tx is still in mempool. cleared when deposit confirms (myReady) or table leaves.
  const [sendTriggered, setSendTriggered] = createSignal(false)
  // payout address the player pastes in the deposit panel; required before "Send with zafu"
  // is enabled. embedded into the deposit memo so the escrow scanner knows where to refund.
  const [payoutOverride, setPayoutOverride] = createSignal<string | null>(null)
  const [selectedTable, setSelectedTable] = createSignal<Table | null>(null)
  const [hasWallet, setHasWallet] = createSignal(false)
  const [walletPubkey, setWalletPubkey] = createSignal<string | undefined>(undefined)
  // staked = real-money table (escrow + on-chain deposits). Set true only with positive
  // evidence (lobby "real money" join, or a server RoomInfo carrying frost DKG coords).
  // Defaults false so free-play/demo never hangs waiting for an escrow that won't come.
  const [staked, setStaked] = createSignal(false)

  // detect zafu wallet via zid SDK
  onCleanup((() => {
    const check = async () => {
      const found = await detectZafu()
      setHasWallet(!!found)
      const id = identity()
      if (id?.sessionPubKey) setWalletPubkey(id.sessionPubKey)
    }
    check()
    const iv = setTimeout(check, 1500)
    return () => clearTimeout(iv)
  })())
  const currentRoom = () => location.pathname.replace(/^\/+|\/+$/g, '')
  // last_session: {room, name} saved on Seated, cleared on Leave — used to detect reconnect
  const initialLastSession = (() => {
    try { return JSON.parse(localStorage.getItem('poker_last_session') || 'null') as { room: string; name: string } | null } catch { return null }
  })()
  const [lastSession, setLastSession] = createSignal(initialLastSession)
  const isReconnect = () => {
    const s = lastSession()
    return !!s && !!currentRoom() && s.room === currentRoom()
  }
  const initialName = (isReconnect() && initialLastSession?.name) || localStorage.getItem('poker_nickname') || ''
  const [name, setName] = createSignal(initialName)
  const [mySeat, setMySeat] = createSignal(-1)
  const [maxSeats, setMaxSeats] = createSignal(2)
  const [oppName, setOppName] = createSignal('\u2014')
  const [oppPubkey, setOppPubkey] = createSignal<string | undefined>(undefined) // peer's persistent zafu identity key
  const [oppVerified, setOppVerified] = createSignal<boolean | undefined>(undefined) // delegation checked out?
  const [playerNames, setPlayerNames] = createSignal<Record<number, string>>({})
  const [stacks, setStacks] = createSignal([0, 0])
  const [bets, setBets] = createSignal([0, 0])
  const [myCards, setMyCards] = createSignal<[CardJson, CardJson] | null>(null)
  const [oppCards, setOppCards] = createSignal<[CardJson, CardJson] | null>(null)
  const [oppRevealed, setOppRevealed] = createSignal(false)
  const [board, setBoard] = createSignal<CardJson[]>([])
  const [pot, setPot] = createSignal(0)
  const [handNum, setHandNum] = createSignal(0)
  const [button, setButton] = createSignal(0)
  const [actions, setActions] = createSignal<ValidAction[]>([])
  const [acting, setActing] = createSignal(-1)
  const [logs, setLogs] = createSignal<{ text: string; cls: string }[]>([])
  const [raiseVal, setRaiseVal] = createSignal(0)
  const [roomCode, setRoomCode] = createSignal('')
  const [inviteUrl, setInviteUrl] = createSignal('')
  const [juryProgress, setJuryProgress] = createSignal('')
  const [escrow, setEscrow] = createSignal('')
  // guard so re-broadcast RoomInfo doesn't spawn a second zafu popup
  let dkgStartedFor = ''
  // guard so repeated DepositStatus(ready) broadcasts start the staked hand only once — and,
  // CRUCIALLY, so a host RELOAD does not re-fire StartHand and re-deal a real-money hand. A
  // plain in-memory boolean resets on reload; persist it per room in localStorage so the deal
  // trigger survives a page refresh. Keyed by room so distinct tables don't collide.
  const stakedStartKey = () => `poker_staked_started:${currentRoom()}`
  const stakedHandStarted = () => {
    try { return !!localStorage.getItem(stakedStartKey()) } catch { return false }
  }
  const markStakedHandStarted = () => {
    try { localStorage.setItem(stakedStartKey(), '1') } catch {}
  }
  // P0: the deposit "already sent" guard MUST survive a page reload. Otherwise refreshing while the
  // first real-ZEC transaction is still unconfirmed re-enables "Send" and a player can deposit
  // twice. Persist it per room and hydrate the in-memory guard from it (mirrors stakedStartKey).
  const depositSentKey = () => `poker_deposit_sent:${currentRoom()}`
  const markDepositSent = () => { try { localStorage.setItem(depositSentKey(), '1') } catch {} }
  createEffect(() => {
    try { if (currentRoom() && localStorage.getItem(depositSentKey())) setSendTriggered(true) } catch {}
  })
  const [pendingRules, setPendingRules] = createSignal<{ buyin: number; smallBlind: number; bigBlind: number; turnTimeout: number; fromSelf: boolean } | null>(null)
  // both players have agreed on the stakes/rules — drives the "agree on stakes" step
  const [rulesAgreed, setRulesAgreed] = createSignal(false)
  // set when we've sat with an opponent present but the hand still hasn't started after a
  // grace period — the handshake likely dropped or the opponent walked away. Drives a clear
  // escape so nobody is ever *permanently* stuck on "waiting for opponent to confirm".
  const [handshakeStuck, setHandshakeStuck] = createSignal(false)
  const [showSettings, setShowSettings] = createSignal(false)
  const [oppDisconnected, setOppDisconnected] = createSignal(false)
  const [reconnectCountdown, setReconnectCountdown] = createSignal(0)
  // tracked so OpponentReconnected / Seated / OpponentLeft can clear the running tick
  let reconnectInterval: ReturnType<typeof setInterval> | null = null
  const clearReconnectInterval = () => {
    if (reconnectInterval !== null) { clearInterval(reconnectInterval); reconnectInterval = null }
  }
  const [actionTimer, setActionTimer] = createSignal(0)
  // highest seconds_left seen this turn ≈ turn length; drives the draining timer bar
  const [timerMax, setTimerMax] = createSignal(0)
  const [autoAction, setAutoAction] = createSignal<'none' | 'check/fold' | 'check' | 'fold' | 'call any'>('none')
  const [deckVerified, setDeckVerified] = createSignal(false)
  const [gameStatus, setGameStatus] = createSignal('')
  const [lastResult, setLastResult] = createSignal<{ won: boolean; amount: number } | null>(null)

  // --- Broadcast (spectator stream) ---
  // player controls what spectators see. server fans out via /{code}/spectate WS.
  // only public info by default. own hole cards opt-in.
  const [broadcasting, setBroadcasting] = createSignal(false)
  const [showMyCards, setShowMyCards] = createSignal(false)
  const [spectatorCount, setSpectatorCount] = createSignal(0)

  function broadcastEvent(event: string, data: Record<string, unknown> = {}) {
    if (!broadcasting()) return
    // filter: only public info
    const payload = JSON.stringify({ event, ...data, ts: Date.now() })
    send({ type: 'Broadcast', data: payload })
  }

  const opp = () => mySeat() === 0 ? 1 : 0
  // an opponent is seated (oppName is reset to the em-dash placeholder when empty/left)
  const oppHere = () => oppName() !== '—'
  const myStack = () => stacks()[mySeat()] ?? 0
  const oppStack = () => stacks()[opp()] ?? 0
  const myBet = () => bets()[mySeat()] ?? 0
  const oppBet = () => bets()[opp()] ?? 0
  const isMyTurn = () => acting() === mySeat() && !oppDisconnected()

  // fold → check → call → bet → raise → allin: the universal client ordering;
  // least-committal left, most aggressive right — regardless of server order
  const ACTION_ORDER: Record<string, number> = { fold: 0, check: 1, call: 2, bet: 3, raise: 4, allin: 5 }
  const sortedActions = () => [...actions()].sort((a, b) => (ACTION_ORDER[a.kind] ?? 9) - (ACTION_ORDER[b.kind] ?? 9))

  // timer bar: % remaining + urgency color (accent → amber → red, pulse in last 5s)
  const timerPct = () => timerMax() > 0 ? Math.min(100, actionTimer() / timerMax() * 100) : 100
  const timerColor = () => timerPct() > 50 ? 'bg-zec-yellow' : timerPct() > 20 ? 'bg-orange-500' : 'bg-red-500'

  const TimerBar = () => (
    <Show when={actionTimer() > 0}>
      <div class="mt-1.5 flex items-center gap-1.5">
        <div class="h-0.75 flex-1 rounded-full bg-white/10 overflow-hidden">
          <div class={`h-full rounded-full transition-[width] duration-1000 ease-linear ${timerColor()}`}
            style={`width:${timerPct()}%`} />
        </div>
        <span class={`font-mono tabular-nums text-10px ${actionTimer() <= 5 ? 'text-red-400 timer-critical' : 'text-white/60'}`}>{actionTimer()}s</span>
      </div>
    </Show>
  )

  function log(text: string, cls = '') {
    setLogs(l => [...l.slice(-60), { text, cls }])
  }

  function onMsg(msg: ServerMsg) {
    switch (msg.type) {
      case 'Seated':
        setMySeat(msg.seat)
        setView('waiting')
        // clear stale connection state — server only sends OpponentDisconnected when an opp
        // *transitions* offline, never a snapshot. On our own reconnect, we'd otherwise carry
        // over any oppDisconnected=true from before our WS bounced.
        clearReconnectInterval()
        setOppDisconnected(false)
        setReconnectCountdown(0)
        // remember the seat so a future visit to this room shows reconnect UI
        if (currentRoom() && msg.name) {
          const s = { room: currentRoom(), name: msg.name }
          localStorage.setItem('poker_last_session', JSON.stringify(s))
          setLastSession(s)
        }
        break
      case 'Waiting':
        setView('waiting')
        break
      case 'OpponentJoined':
        setOppName(msg.name)
        setPlayerNames(p => ({ ...p, [msg.seat]: msg.name }))
        if (msg.pubkey !== undefined) setOppPubkey(msg.pubkey)
        if (msg.verified !== undefined) setOppVerified(msg.verified)
        break
      case 'RulesProposed':
        setPendingRules({ buyin: msg.buyin, smallBlind: msg.smallBlind, bigBlind: msg.bigBlind, turnTimeout: (msg as any).turnTimeout ?? 30, fromSelf: msg.fromSelf })
        if (msg.fromSelf) {
          log(`proposed: ${msg.smallBlind}/${msg.bigBlind} blinds, ${msg.buyin} buyin`)
        } else {
          log(`opponent proposes: ${msg.smallBlind}/${msg.bigBlind} blinds, ${msg.buyin} buyin`, 'c-zec-yellow')
        }
        break
      case 'RulesAccepted':
        setPendingRules(null)
        setRulesAgreed(true)
        log('rules accepted', 'c-green')
        break
      case 'OpponentLeft':
        clearReconnectInterval()
        setOppDisconnected(false)
        setReconnectCountdown(0)
        setOppName('\u2014')
        setOppPubkey(undefined)
        setOppVerified(undefined)
        setRulesAgreed(false)
        setPendingRules(null)
        setActions([])
        // close the P2P media channel + stop our own cam/mic \u2014 the peer is gone for good, so
        // the RTCPeerConnection is dead and leaving the camera light on is a privacy footgun.
        media()?.cleanup()
        setView('waiting')
        log('opponent left')
        break
      case 'OpponentDisconnected': {
        // clear any prior countdown so we don't end up with two racing intervals
        clearReconnectInterval()
        setOppDisconnected(true)
        setReconnectCountdown(msg.reconnect_secs)
        log(`opponent disconnected (${msg.reconnect_secs}s to reconnect)`, 'c-red')
        reconnectInterval = setInterval(() => {
          setReconnectCountdown(c => {
            if (c <= 1) {
              clearReconnectInterval()
              // Safety net: the server normally broadcasts OpponentLeft when the reconnect
              // window expires, which tears the table down. But if that frame is dropped we'd
              // sit on a frozen "0s" table forever. Give it a short grace, then if the opponent
              // still hasn't returned, locally treat them as gone (reuses the OpponentLeft path,
              // which also closes the media channel).
              setTimeout(() => {
                if (oppDisconnected()) onMsg({ type: 'OpponentLeft', seat: opp() })
              }, 5000)
              return 0
            }
            return c - 1
          })
        }, 1000)
        break
      }
      case 'OpponentReconnected':
        clearReconnectInterval()
        setOppDisconnected(false)
        setReconnectCountdown(0)
        log('opponent reconnected', 'c-green')
        break
      case 'TimerTick':
        setActionTimer(msg.seconds_left)
        if (msg.seconds_left > timerMax()) setTimerMax(msg.seconds_left)
        break
      case 'ActionPaused':
        // freeze the visible action timer; the OPPONENT DISCONNECTED overlay covers the table
        setActionTimer(0)
        log(`hand paused while seat ${msg.seat} is offline`, 'c-zec-yellow')
        break
      case 'ActionResumed':
        setActionTimer(msg.seconds_left)
        log(`hand resumed (seat ${msg.seat} to act, ${msg.seconds_left}s)`, 'c-green')
        break
      case 'ActionTimeout':
        log(`${msg.seat === mySeat() ? 'you' : 'opp'} timed out (auto-fold)`, 'c-red')
        setActionTimer(0)
        break
      case 'HandStarted':
        setView('game')
        setStacks(msg.stacks)
        setButton(msg.button)
        setHandNum(msg.hand_number)
        setBoard([])
        setPot(0)
        setActions([])
        setGameStatus('') // clear shuffle overlay
        setOppRevealed(false)
        setOppCards(null)
        if (msg.your_cards) {
          setMyCards(msg.your_cards)
        }
        log(`hand #${msg.hand_number}`, 'c-green')
        broadcastEvent('hand_started', {
          hand: msg.hand_number, button: msg.button, stacks: msg.stacks,
          ...(showMyCards() && msg.your_cards ? { hero_cards: msg.your_cards } : {}),
        })
        break
      case 'BlindsPosted':
        setBets(b => {
          const n = [...b]
          n[msg.small_blind[0]] = msg.small_blind[1]
          n[msg.big_blind[0]] = msg.big_blind[1]
          return n
        })
        break
      case 'ActionRequired':
        if (msg.seat < 0) { setActions([]); setActing(-1); break } // clear stale
        setActing(msg.seat)
        setTimerMax(0) // new turn — relearn the bar scale from the first tick
        if (msg.seat === mySeat()) {
          // don't auto-action when all-in (stack = 0) or only 1 valid action
          const myCurrentStack = stacks()[mySeat()] ?? 0
          if (myCurrentStack === 0) {
            setActions(msg.valid_actions)
            break
          }
          // check auto-action first
          const aa = autoAction()
          const hasCheck = msg.valid_actions.some(a => a.kind === 'check')
          const hasCall = msg.valid_actions.find(a => a.kind === 'call')
          let autoFired = false
          if (aa === 'check/fold') {
            setAutoAction('none')
            // T3: use setTimeout(0) to let UI update before action
            setTimeout(() => { if (hasCheck) act('check'); else act('fold') }, 0)
            autoFired = true
          } else if (aa === 'check' && hasCheck) {
            setAutoAction('none')
            setTimeout(() => act('check'), 0)
            autoFired = true
          } else if (aa === 'fold') {
            setAutoAction('none')
            setTimeout(() => act('fold'), 0)
            autoFired = true
          } else if (aa === 'call any' && hasCall) {
            setAutoAction('none')
            setTimeout(() => act('call', hasCall.min_amount), 0)
            autoFired = true
          }
          if (autoFired) break
          // auto-action didn't match this prompt — keep it active for next turn
          setActions(msg.valid_actions)
          const r = msg.valid_actions.find(a => a.kind === 'raise' || a.kind === 'bet')
          if (r) setRaiseVal(r.min_amount)
        } else {
          setActions([])
        }
        break
      case 'PlayerActed': {
        setActing(-1)
        setActions([])
        const stackBefore = stacks()[msg.seat] ?? 0
        const s = [...stacks()]
        s[msg.seat] = msg.new_stack
        setStacks(s)
        const increment = stackBefore - msg.new_stack
        const b = [...bets()]
        if (increment > 0) b[msg.seat] = (b[msg.seat] ?? 0) + increment
        setBets(b)
        const pos = msg.seat === button() ? 'BTN' : 'BB'
        const who = msg.seat === mySeat() ? `you(${pos})` : `opp(${pos})`
        const amt = msg.amount > 0 && (msg.action === 'bet' || msg.action === 'raise') ? ` ${msg.amount}` : ''
        log(`${who}: ${msg.action}${amt}`)
        broadcastEvent('action', { seat: msg.seat, action: msg.action, amount: msg.amount })
        break
      }
      case 'CommunityCards':
        setBoard(msg.cards)
        setBets([0, 0])
        log(`${msg.phase}: ${msg.cards.map(c => c.rank + c.suit).join(' ')}`, 'c-green')
        broadcastEvent('community', { phase: msg.phase, cards: msg.cards })
        break
      case 'PotUpdate':
        setPot(msg.pots.reduce((s, p) => s + p.amount, 0))
        break
      case 'Showdown':
        for (const [seat, cards] of msg.hands) {
          if (seat === opp()) { setOppCards(cards); setOppRevealed(true) }
        }
        log('showdown', 'c-green')
        broadcastEvent('showdown', { hands: msg.hands })
        break
      case 'PotAwarded': {
        const won = msg.seat === mySeat()
        log(`${won ? 'you' : oppName()} wins ${msg.amount}${msg.amount === 0 ? ' (split)' : ''}`, 'c-zec-yellow font-500')
        setLastResult({ won, amount: msg.amount })
        setTimeout(() => setLastResult(null), 2500)
        broadcastEvent('pot_awarded', { seat: msg.seat, amount: msg.amount })
        break
      }
      case 'HandComplete':
        setStacks(msg.stacks)
        setBets([0, 0])
        setActions([])
        setActing(-1)
        break
      case 'Chat':
        // chat arrives from the P2P transport as { from:'opp', text } (ws.ts) — msg.name/seat
        // don't exist here, which rendered "undefined:". Show the peer label instead.
        log(`${(msg as any).from === 'opp' ? 'opponent' : ((msg as any).name ?? 'opponent')}: ${msg.text}`, 'text-neutral-300')
        break
      case 'JuryVote':
        setJuryProgress(`jury ${msg.node}/${msg.total}`)
        log(`jury node ${msg.node}/${msg.total} voted [${msg.payload_hash}]`, 'c-green')
        break
      case 'JurySettlement':
        setJuryProgress('')
        log(`settlement ${msg.verified ? 'verified' : 'FAILED'} (${msg.threshold}/${msg.contributions} OSST)`, msg.verified ? 'c-green font-500' : 'c-red')
        break
      case 'RoomInfo':
        if (msg.code && msg.code !== roomCode()) {
          setRoomCode(msg.code)
          setLogs([])
          log(`table: ${msg.code}`, 'c-green')
        }
        if (msg.escrow && msg.escrow.length > 5) {
          setEscrow(msg.escrow)
        }
        // Real-money detection. The server now sends an explicit `staked` boolean on RoomInfo
        // (true ONLY for real-money tables) — trust it verbatim when present. When it's absent
        // (older relay), fall back to the FROST-coords / real-escrow-UA heuristic. The `u1mock…`
        // free-play mock escrow (>5 chars, from negotiate.ts) is excluded from the fallback so a
        // FREE table can't get flipped into the staked "deposit to vault" flow.
        if (typeof msg.staked === 'boolean') {
          setStaked(msg.staked)
        } else if ((msg.frost_relay_url && msg.frost_room_code) || (msg.escrow && msg.escrow.length > 5 && !msg.escrow.startsWith('u1mock'))) {
          setStaked(true)
        }
        if (msg.required_deposit) setRequiredDeposit(msg.required_deposit)
        if (typeof msg.buyin_zat === 'number') setDepositBuyinZat(msg.buyin_zat)
        if (typeof msg.fee_per_seat === 'number') setDepositFeePerSeat(msg.fee_per_seat)
        if (typeof msg.frost_relay_url === 'string') setSettleFrostRelay(msg.frost_relay_url)
        if (msg.seat_addresses && msg.seat_addresses.length > 0) {
          setSeatAddresses(msg.seat_addresses)
        }
        // cache-on-fire: reload mid-DKG must not spawn a second zafu popup conflicting with the first
        const dkgFiredKey = `poker_dkg_fired:${msg.code}`
        const dkgAlreadyFired = !!localStorage.getItem(dkgFiredKey)
        console.log('[poker-dkg] RoomInfo received:', {
          code: msg.code,
          frost_relay_url: msg.frost_relay_url,
          frost_room_code: msg.frost_room_code,
          escrow: msg.escrow,
          escrowEmpty: !msg.escrow || msg.escrow.length === 0,
          dkgStartedFor,
          dkgAlreadyFired,
          willFire: !!(msg.frost_relay_url && msg.frost_room_code && (!msg.escrow || msg.escrow.length === 0) && dkgStartedFor !== msg.code && !dkgAlreadyFired),
        })
        if (
          msg.frost_relay_url && msg.frost_room_code &&
          (!msg.escrow || msg.escrow.length === 0) &&
          dkgStartedFor !== msg.code &&
          !dkgAlreadyFired
        ) {
          dkgStartedFor = msg.code
          localStorage.setItem(dkgFiredKey, '1')
          log('setting up multisig escrow (zafu)...', 'c-zec-yellow')
          void requestPokerDkg({
            relayUrl: msg.frost_relay_url,
            roomCode: msg.frost_room_code,
            threshold: 2,
            maxSigners: 3,
            labelPrefix: `POKER-${msg.code}`,
          }).then(res => {
            if (res.success) {
              log(`multisig ready: ${res.address.slice(0, 16)}…`, 'c-green')
              send({ type: 'DkgComplete', escrow_ua: res.address, orchard_fvk: res.orchardFvk })
            } else {
              log(`multisig setup failed: ${res.error}`, 'c-red')
              // report to the escrow journal — the server/escrow can't otherwise see
              // that THIS client's DKG ceremony failed. lets operators triage a stuck room.
              send({ type: 'EscrowFault', phase: 'dkg', detail: `multisig setup failed: ${res.error ?? 'unknown'}` })
              localStorage.removeItem(dkgFiredKey)
              dkgStartedFor = ''
            }
          }).catch(err => {
            log(`multisig setup error: ${err}`, 'c-red')
            send({ type: 'EscrowFault', phase: 'dkg', detail: `multisig setup threw: ${err}` })
            localStorage.removeItem(dkgFiredKey)
            dkgStartedFor = ''
          })
        } else if (dkgAlreadyFired && (!msg.escrow || msg.escrow.length === 0)) {
          dkgStartedFor = msg.code
          log('multisig setup in progress — waiting for escrow…', 'c-zec-yellow')
        }
        break
      case 'DepositStatus': {
        if (msg.seat_addresses && msg.seat_addresses.length > 0) {
          setSeatAddresses(msg.seat_addresses)
        }
        if (msg.escrow_address) setEscrow(msg.escrow_address)
        setDepositA(msg.player_a_deposit)
        setDepositB(msg.player_b_deposit)
        setPendingA(msg.player_a_pending ?? 0)
        setPendingB(msg.player_b_pending ?? 0)
        setRequiredDeposit(msg.required)
        setDepositReady(msg.ready)
        // once DKG produced seat addresses + deposits still pending, sit users in the deposit
        // view. Do NOT flip away from settlement (payout in flight) or game (active hand).
        const v = view()
        if (mySeat() >= 0 && !msg.ready && v !== 'game' && v !== 'settlement' && msg.seat_addresses && msg.seat_addresses.some(a => a)) {
          setView('deposit')
        }
        // STAKED tables: the first hand must NOT start until both deposits are confirmed
        // on-chain. The server sends DepositStatus{ready:true} only when both seats are funded.
        // Host (seat 0) starts the deal exactly once; the guest follows via the P2P deal message.
        // Free-play tables ignore this path — they start immediately via negotiate.onReady.
        if (staked() && msg.ready && !stakedHandStarted() && mySeat() === 0 && v !== 'settlement') {
          markStakedHandStarted()
          log('both deposits confirmed — dealing', 'c-green')
          send({ type: 'StartHand' })
        }
        break
      }
      case 'GameOver': {
        const myPayout = msg.payouts.find((p: any) => p[0] === mySeat())
        log(`game over: ${msg.reason}`, 'c-red font-500')
        if (myPayout) log(`your payout: ${myPayout[1]}`, 'c-zec-yellow font-500')
        setActions([])
        setActing(-1)
        setSettleReason(msg.reason)
        // Free tournament auto-report: report the WINNING handle so the bracket advances. Both
        // players report; the backend only advances when they agree. Winner = seat with the
        // largest payout; opponent name is stored formatted ("name (mode)") so strip the suffix.
        try {
          if (Array.isArray(msg.payouts) && msg.payouts.length) {
            const winSeat = msg.payouts.reduce((b: any, p: any) => (p[1] > b[1] ? p : b), msg.payouts[0])[0]
            const rawOpp = (playerNames()[winSeat] || '').replace(/\s*\([^)]*\)\s*$/, '')
            const winnerName = winSeat === mySeat() ? (localStorage.getItem('poker_nickname') || rawOpp) : rawOpp
            if (winnerName) reportTournamentResult(winnerName)
          }
        } catch { /* best-effort; non-tournament games have no stashed match */ }
        // real tables: flip to settlement immediately with a 'preparing' spinner. PCZT build
        // + relay-room provisioning takes ~3-5s so GameOver lands well before PayoutSigningRequest.
        // Pre-populate the plan from GameOver.payouts so the user sees per-seat amounts right away;
        // addresses get filled in by the real PayoutSigningRequest a few seconds later.
        if (escrow() && escrow().length > 0) {
          const planPreview = msg.payouts.map(([seat, amt]: [number, number]) => ({
            seat, address: '', amount_zat: amt,
          }))
          setSettlePlan(planPreview)
          setSettlePrioritySeat(-1) // unknown until PayoutSigningRequest
          setSettleStatus({ phase: 'preparing' })
          setView('settlement')
        }
        break
      }
      case 'PayoutSigningRequest': {
        log(`settling: ${msg.plan.map(p => `seat${p.seat}=${p.amount_zat}`).join(', ')}`, 'c-zec-yellow')
        setSettleRelayRoom(msg.relay_room)
        setSettlePlan(msg.plan)
        setSettlePrioritySeat(msg.priority_seat)
        setSettleStatus({ phase: 'pending' })
        // server is the source of truth for the fallback timer — it sends remaining seconds
        // so a reconnect mid-wait resumes the correct value instead of restarting at 90
        const remaining = Math.max(0, Math.min(SETTLE_FALLBACK_SECS, msg.fallback_secs_remaining ?? SETTLE_FALLBACK_SECS))
        setSettleBroadcastAt(Date.now() - (SETTLE_FALLBACK_SECS - remaining) * 1000)
        setSettleFallbackTick(remaining)
        setView('settlement')
        setActions([])
        setActing(-1)
        break
      }
      case 'OpponentAbandoned': {
        log(`opponent (seat ${msg.seat}) disconnected and didn't return`, 'c-red')
        setSettleReason(`seat ${msg.seat} abandoned`)
        // PayoutSigningRequest follows from the server's auto-settle path; view-flip happens there
        break
      }
      case 'PayoutComplete': {
        log(`✓ paid out: tx ${msg.txid}`, 'c-green font-500')
        setSettleStatus({ phase: 'complete', txid: msg.txid })
        setView('settlement')
        // settlement done — drop the reconnect marker; the "return to lobby" button can
        // safely take the user home without auto-rejoining a settled room
        localStorage.removeItem('poker_last_session')
        localStorage.removeItem(`poker_dkg_fired:${roomCode()}`)
        localStorage.removeItem(`poker_staked_started:${roomCode()}`)
        localStorage.removeItem(`poker_deposit_sent:${roomCode()}`)
        setLastSession(null)
        // schedule deletion of the multisig vault 24h from now — it's spent + useless
        void requestDeletePokerMultisig({
          multisigLabel: `POKER-${roomCode()}`,
          delayMs: 24 * 60 * 60 * 1000,
        })
        break
      }
      case 'PayoutFailed': {
        log(`✗ payout failed: ${msg.reason}`, 'c-red font-500')
        setSettleStatus({ phase: 'failed', reason: msg.reason })
        setView('settlement')
        break
      }
      case 'InviteLink':
        setInviteUrl(window.location.origin + msg.url)
        break
      case 'Status':
        setGameStatus(msg.message)
        if (msg.message.includes('verified')) setDeckVerified(true)
        if (msg.phase === 'dealing') setDeckVerified(false) // reset for new hand
        break
      case 'Error':
        log(`err: ${msg.message}`)
        // relay unreachable / lost → open the relay picker so the player can switch nodes
        // instead of being stranded on a dead relay ("connecting…" forever).
        if (/relay unreachable|lost connection to relay/i.test(msg.message || '')) {
          setShowSettings(true)
        }
        break
    }
  }

  const { connected, connect, send, identity, encrypted, media } = createSocket(onMsg)

  // WebRTC voice/video is hard opt-in: unlike the game (P2P over the blind
  // relay, no IP leak), media connects directly peer-to-peer and reveals each
  // player's IP to the other. We show an acknowledgement dialog before the very
  // first getUserMedia / RTCPeerConnection. `pendingMedia` records which control
  // the user clicked so we can resume it after they consent.
  const [showMediaAck, setShowMediaAck] = createSignal(false)
  // 'mic'/'cam': the local control the user clicked. 'incoming': the user is
  // responding to the opponent's offer (accepting connects the mic by default).
  const [pendingMedia, setPendingMedia] = createSignal<'mic' | 'cam' | 'incoming' | null>(null)
  // True when the browser's autoplay policy blocked the REMOTE video from
  // playing (it carries the opponent's audio, which browsers refuse to autoplay
  // without a user gesture). We surface a "tap to play" overlay so the user is
  // never stuck at a black tile; the tap is the gesture that unblocks it.
  const [remoteNeedsTap, setRemoteNeedsTap] = createSignal(false)

  // Bind a MediaStream to a <video> and drive playback explicitly instead of
  // trusting the `autoplay` attribute. Two reasons this matters here:
  //   1. ontrack builds a FRESH MediaStream on every renegotiation (the cam
  //      re-attach fix). prop:srcObject updates reactively, but a reactive
  //      srcObject swap does NOT re-trigger autoplay, so a video track added
  //      after an audio-only call would never paint → the "camera black" bug.
  //   2. A remote stream with audio is blocked by autoplay policy unless muted;
  //      we must NOT mute the remote (we want to hear the opponent), so play()
  //      can reject → we flip a "tap to play" flag instead of silently failing.
  // `muted` mutes only the LOCAL preview (echo cancellation); the remote is live.
  function bindVideo(
    el: HTMLVideoElement,
    stream: () => MediaStream | null,
    opts: { muted: boolean; onBlocked?: (blocked: boolean) => void },
  ) {
    el.muted = opts.muted
    el.autoplay = true
    el.playsInline = true
    createEffect(() => {
      const s = stream()
      // Re-bind on every new reference (a new track was added/swapped).
      if (el.srcObject !== s) el.srcObject = s
      if (!s) { opts.onBlocked?.(false); return }
      // Explicitly (re)start playback after the bind. If the policy blocks it
      // (remote audio, no gesture yet), report it so the UI shows a tap overlay.
      el.play()
        .then(() => opts.onBlocked?.(false))
        .catch(() => opts.onBlocked?.(true))
    })
  }

  // Where each floating video tile sits (px from top-left of viewport). Persisted
  // for the session so a tile the user parked stays put across re-renders. null =
  // "not placed yet" → CSS default corner is used until first drag.
  const [remotePos, setRemotePos] = createSignal<{ x: number; y: number } | null>(null)
  const [localPos, setLocalPos] = createSignal<{ x: number; y: number } | null>(null)

  // Make a floating panel draggable by pointer. Grabbing anywhere on the panel
  // moves it (except elements marked data-nodrag, e.g. the resize corner / play
  // button). Uses pointer capture so the drag survives fast moves and leaving the
  // element, and clamps to the viewport so a tile can never be lost off-screen.
  function makeDraggable(
    el: HTMLElement,
    setPos: (p: { x: number; y: number }) => void,
  ) {
    let startX = 0, startY = 0, originX = 0, originY = 0, dragging = false
    const onDown = (e: PointerEvent) => {
      // Ignore drags that start on the native resize corner or opt-out children.
      if ((e.target as HTMLElement)?.closest('[data-nodrag]')) return
      // Only start a drag from empty tile chrome, never from the resize handle:
      // the browser's ::-webkit-resizer sits in the bottom-right ~16px.
      const r = el.getBoundingClientRect()
      if (e.clientX > r.right - 18 && e.clientY > r.bottom - 18) return
      originX = r.left; originY = r.top
      startX = e.clientX; startY = e.clientY
      dragging = true
      el.setPointerCapture(e.pointerId)
    }
    const onMove = (e: PointerEvent) => {
      if (!dragging) return
      const dx = e.clientX - startX, dy = e.clientY - startY
      const w = el.offsetWidth, h = el.offsetHeight
      const x = Math.min(Math.max(0, originX + dx), window.innerWidth - w)
      const y = Math.min(Math.max(0, originY + dy), window.innerHeight - h)
      setPos({ x, y })
    }
    const onUp = (e: PointerEvent) => {
      dragging = false
      try { el.releasePointerCapture(e.pointerId) } catch { /* ignore */ }
    }
    el.addEventListener('pointerdown', onDown)
    el.addEventListener('pointermove', onMove)
    el.addEventListener('pointerup', onUp)
    el.addEventListener('pointercancel', onUp)
  }

  // Entry point for the mic/cam buttons: if the user has already acknowledged,
  // toggle immediately; otherwise open the consent dialog and remember intent.
  function requestMedia(kind: 'mic' | 'cam') {
    const m = media()
    if (!m) return
    if (m.acknowledged()) {
      kind === 'mic' ? m.toggleMic() : m.toggleCam()
      return
    }
    setPendingMedia(kind)
    setShowMediaAck(true)
  }

  // The opponent started media (an offer arrived) but we haven't opted in. Route
  // the "accept" action through the SAME acknowledgement dialog: only after the
  // user consents does any RTCPeerConnection get created on our side.
  function acceptIncomingMedia() {
    const m = media()
    if (!m) return
    setPendingMedia('incoming')
    setShowMediaAck(true)
  }

  function confirmMediaAck() {
    const m = media()
    const kind = pendingMedia()
    setShowMediaAck(false)
    setPendingMedia(null)
    if (!m) return
    m.acknowledge()
    if (kind === 'mic' || kind === 'incoming') m.toggleMic()
    else if (kind === 'cam') m.toggleCam()
  }

  function cancelMediaAck() {
    setShowMediaAck(false)
    setPendingMedia(null)
  }

  function sit() {
    const n = name().trim() || 'anon' + String(Math.random() * 100000 | 0).padStart(5, '0')
    // read custom rules from inputs (host only)
    const sbEl = document.getElementById('sb-input') as HTMLInputElement | null
    const bbEl = document.getElementById('bb-input') as HTMLInputElement | null
    const buyinEl = document.getElementById('buyin-input') as HTMLInputElement | null
    const timeoutEl = document.getElementById('timeout-input') as HTMLInputElement | null
    const customRules = sbEl ? {
      smallBlind: parseInt(sbEl.value) || 5,
      bigBlind: parseInt(bbEl?.value ?? '10') || 10,
      buyin: parseInt(buyinEl?.value ?? '1000') || 1000,
      turnTimeout: parseInt(timeoutEl?.value ?? '30') || 30,
    } : undefined
    connect(n, customRules, staked())
  }

  // no auto-connect — always show lobby so user can enter name

  function act(action: string, amount?: number) {
    send({ type: 'Action', action, ...(amount !== undefined && { amount }) })
    setActions([])
  }

  function leaveTable() {
    if (!confirm('Leave table and cash out?')) return
    send({ type: 'Leave' })
    setActions([])
    setActing(-1)
    // KEEP `poker_last_session` so a reload during settlement triggers reconnect (server
    // replays PayoutSigningRequest into the settlement view rather than dumping us at the
    // rename UI). PayoutComplete / PayoutFailed handlers + the manual "return to lobby"
    // button clear it. For bot tables we still want a quick bounce to lobby; the 5s
    // fallback below handles that.
    setTimeout(() => {
      if (view() !== 'settlement') {
        // bot table or server didn't open a settlement → safe to scrub session + bounce
        localStorage.removeItem('poker_last_session')
        localStorage.removeItem(`poker_dkg_fired:${roomCode()}`)
        localStorage.removeItem(`poker_staked_started:${roomCode()}`)
        localStorage.removeItem(`poker_deposit_sent:${roomCode()}`)
        // drop the reconnect seat pin(s) for this room — leaving is intentional, so a future
        // visit should get a fresh role rather than reclaim this one.
        const seatPrefix = `poker_seat:${roomCode()}:`
        for (let i = localStorage.length - 1; i >= 0; i--) {
          const k = localStorage.key(i)
          if (k && k.startsWith(seatPrefix)) localStorage.removeItem(k)
        }
        setLastSession(null)
        history.pushState(null, '', '/')
        setView('casino')
      }
    }, 5000)
  }

  // keybinding modes
  const [keyMode, setKeyMode] = createSignal<'classic' | 'vim'>('classic')

  // keyboard shortcuts
  onCleanup((() => {
    const onKey = (e: KeyboardEvent) => {
      if (view() !== 'game') return
      if ((e.target as HTMLElement)?.tagName === 'INPUT') return

      const a = actions()
      const myTurn = isMyTurn() && a.length > 0
      const hasBet = a.some(v => v.kind === 'bet' || v.kind === 'raise')
      const betAction = a.find(v => v.kind === 'raise') || a.find(v => v.kind === 'bet')

      if (!myTurn) return

      const km = keyMode()

      if (km === 'classic') {
        // PokerStars-style: F1-F4, Space, Enter, Q, 1-4
        switch (e.key) {
          case 'F1': e.preventDefault(); if (a.some(v => v.kind === 'fold')) act('fold'); break
          case 'F2': e.preventDefault()
            if (a.some(v => v.kind === 'check')) act('check')
            else if (a.some(v => v.kind === 'call')) act('call'); break
          case 'F3': e.preventDefault()
            if (betAction) act(betAction.kind, raiseVal() || betAction.min_amount); break
          case 'F4': e.preventDefault()
            if (hasBet && betAction) act(betAction.kind, pot()); break
          case ' ': e.preventDefault()
            if (a.some(v => v.kind === 'check')) act('check')
            else if (a.some(v => v.kind === 'call')) act('call'); break
          case 'Enter': e.preventDefault()
            if (betAction && raiseVal() > 0) act(betAction.kind, raiseVal()); break
          case 'q': case 'Q':
            if (a.some(v => v.kind === 'allin')) act('allin'); break
        }
      } else {
        // vim-style: hjkl movement metaphor
        // f=fold  d=check(do nothing)  s=call(see)  r=raise  w=bet(wager)
        // a=allin  e=enter(confirm raise)  x=check/fold preset
        // gg=min raise  G=pot  H=½pot  L=2x  M=¾pot
        switch (e.key) {
          case 'f': if (a.some(v => v.kind === 'fold')) act('fold'); break
          case 'd': if (a.some(v => v.kind === 'check')) act('check'); break
          case 's': if (a.some(v => v.kind === 'call')) act('call')
                    else if (a.some(v => v.kind === 'check')) act('check'); break
          case 'w': if (betAction) act(betAction.kind, raiseVal() || betAction.min_amount); break
          case 'r': if (betAction) act(betAction.kind, raiseVal() || betAction.min_amount); break
          case 'a': if (a.some(v => v.kind === 'allin')) act('allin'); break
          case 'e': case 'Enter':
            if (betAction && raiseVal() > 0) act(betAction.kind, raiseVal()); break
          case 'x': // check/fold toggle
            setAutoAction(autoAction() === 'check/fold' ? 'none' : 'check/fold'); break
          case 'G': if (hasBet) setRaiseVal(pot()); break
          case 'H': if (hasBet && betAction) setRaiseVal(Math.max(Math.floor(pot() / 2), betAction.min_amount)); break
          case 'M': if (hasBet && betAction) setRaiseVal(Math.max(Math.floor(pot() * 3 / 4), betAction.min_amount)); break
          case 'L': if (hasBet && betAction) setRaiseVal(Math.max(pot() * 2, betAction.min_amount)); break
          case 'j': if (hasBet) setRaiseVal(v => Math.max(v - (betAction?.min_amount || 10), betAction?.min_amount || 0)); break
          case 'k': if (hasBet) setRaiseVal(v => v + (betAction?.min_amount || 10)); break
        }
      }

      // number sizing works in both modes
      if (hasBet && betAction) {
        const p = pot() || 1
        const min = betAction.min_amount || 0
        switch (e.key) {
          case '1': setRaiseVal(Math.max(Math.floor(p / 4), min)); break   // 1/4 pot
          case '2': setRaiseVal(Math.max(Math.floor(p / 2), min)); break   // 1/2 pot
          case '3': setRaiseVal(Math.max(Math.floor(p * 3 / 4), min)); break // 3/4 pot
          case '4': setRaiseVal(Math.max(p, min)); break                    // pot
          case '5': setRaiseVal(Math.max(p * 2, min)); break               // 2x pot
        }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })())

  // action timer countdown — driven by game.ts authoritative timer (T1 fix)

  // auto-scroll log
  let logEl!: HTMLDivElement
  createEffect(() => {
    logs()
    if (logEl) logEl.scrollTop = logEl.scrollHeight
  })

  // Watchdog: if we linger on the waiting screen with an opponent present, the rules
  // handshake was probably dropped (ws.ts auto-retries for ~20s first). Surface an escape.
  createEffect(() => {
    const lingering = view() === 'waiting' && oppHere()
    setHandshakeStuck(false)
    if (!lingering) return
    const t = setTimeout(() => {
      if (view() === 'waiting' && oppHere()) setHandshakeStuck(true)
    }, 24000)
    onCleanup(() => clearTimeout(t))
  })

  // ── agreement / handshake tracker ────────────────────────────────────────────
  // Honest, shared view of where the two players are before the deal: joined →
  // stakes agreed → (if real) escrow funded → deal. Reflects real signals, not a
  // fake spinner, so both sides always know what they're waiting on.
  type StepState = 'done' | 'active' | 'pending'
  // ZEC amount formatter for the tracker (zats → human ZEC)
  const fmtZecAmt = (zats: number) => {
    const z = zats / 1e8
    return (z >= 1 ? z.toFixed(1) : z >= 0.01 ? z.toFixed(2) : z.toFixed(4)).replace(/\.?0+$/, '') + ' ZEC'
  }
  // best available buy-in (zats): live escrow figure, else the tier we chose, else the pot rules
  const buyinZat = () => depositBuyinZat() || selectedTable()?.buyin || 0
  function agreementSteps(): { label: string; sub?: string; state: StepState }[] {
    const steps: { label: string; sub?: string; state: StepState }[] = []
    steps.push({ label: 'Table created', state: 'done' })
    steps.push({
      label: oppHere() ? `${oppName()} joined` : 'Waiting for opponent',
      sub: oppHere() ? undefined : 'share the invite link below',
      state: oppHere() ? 'done' : 'active',
    })
    if (staked()) {
      // escrow setup beginning (addresses issued / deposit view) implies stakes settled,
      // so the step never hangs even if the flow skips an explicit rules round-trip.
      const stakesDone = rulesAgreed() || seatAddresses().some(a => a) || view() === 'deposit'
      const buyinLabel = buyinZat() ? `${fmtZecAmt(buyinZat())} buy-in` : 'the buy-in'
      steps.push({
        label: 'Agree on stakes',
        sub: pendingRules() && !pendingRules()!.fromSelf
          ? `${buyinLabel} — needs your OK`
          : stakesDone ? buyinLabel
          : `${buyinLabel} — both players confirm`,
        state: !oppHere() ? 'pending' : stakesDone ? 'done' : 'active',
      })
      const req = requiredDeposit()
      const myDep = mySeat() === 0 ? depositA() : depositB()
      const oppDep = mySeat() === 0 ? depositB() : depositA()
      const bothFunded = req > 0 && myDep >= req && oppDep >= req
      steps.push({
        label: 'Fund escrow',
        sub: req > 0
          ? `you ${myDep >= req ? '✓ funded' : '… pending'} · opponent ${oppDep >= req ? '✓ funded' : '… pending'}`
          : 'each player deposits their buy-in to the 2-of-3 vault',
        state: !stakesDone ? 'pending' : bothFunded ? 'done' : 'active',
      })
      steps.push({ label: 'Deal', state: bothFunded ? 'active' : 'pending' })
    } else {
      steps.push({ label: 'Free play — nothing at stake', state: oppHere() ? 'done' : 'pending' })
      steps.push({ label: 'Deal', state: oppHere() ? 'active' : 'pending' })
    }
    return steps
  }

  const AgreementTracker = () => (
    <div class="mx-auto max-w-sm text-left mb-5">
      <div class="flex items-center justify-between mb-2">
        <span class="text-neutral-500 text-10px uppercase tracking-wider">agreement</span>
        <span class={`text-10px uppercase tracking-wider ${staked() ? 'text-zec-yellow' : 'text-neutral-500'}`}>
          {staked() ? `real ZEC${buyinZat() ? ` · ${fmtZecAmt(buyinZat())}` : ''}` : 'free play'}
        </span>
      </div>
      <div class="flex flex-col">
        <For each={agreementSteps()}>
          {(s, i) => (
            <div class="flex items-start gap-2.5 py-1">
              <div class="flex flex-col items-center self-stretch">
                <div class={`w-4 h-4 rounded-full flex items-center justify-center shrink-0 text-9px ${
                  s.state === 'done' ? 'bg-green-500 text-black'
                  : s.state === 'active' ? 'border-2 border-zec-yellow text-zec-yellow animate-pulse'
                  : 'border border-white/15 text-transparent'
                }`}>{s.state === 'done' ? '✓' : '•'}</div>
                <Show when={i() < agreementSteps().length - 1}>
                  <div class={`w-px flex-1 min-h-3 ${s.state === 'done' ? 'bg-green-500/40' : 'bg-white/10'}`} />
                </Show>
              </div>
              <div class="pb-1">
                <div class={`text-12px leading-tight ${
                  s.state === 'done' ? 'text-white/80' : s.state === 'active' ? 'text-zec-yellow' : 'text-neutral-600'
                }`}>{s.label}</div>
                <Show when={s.sub}>
                  <div class="text-10px text-neutral-500 leading-tight mt-0.5">{s.sub}</div>
                </Show>
              </div>
            </div>
          )}
        </For>
      </div>
    </div>
  )

  // Compact table chat for the pre-game states (waiting / deposit). Same P2P chat pipe as
  // the in-hand sidebar — social from the moment both players are seated, not just mid-hand.
  let preChatEl: HTMLDivElement | undefined
  createEffect(() => { logs(); if (preChatEl) preChatEl.scrollTop = preChatEl.scrollHeight })
  const TableChat = () => (
    <div class="mx-auto max-w-sm mt-5 border border-white/10 rounded-lg overflow-hidden text-left">
      <div class="px-2.5 py-1 bg-neutral-900/50 border-b border-white/10 text-10px text-neutral-400 uppercase tracking-wider">
        chat with {oppName()}
      </div>
      <div ref={el => (preChatEl = el)} class="h-24 overflow-y-auto px-2 py-1 font-mono text-10px bg-zec-surface/50 leading-relaxed">
        <For each={logs().slice(-40)}>
          {l => <div class={`text-neutral-600 ${l.cls}`}>{l.text}</div>}
        </For>
      </div>
      <div class="flex gap-1 px-2 py-1 flex-wrap border-t border-white/10">
        {['hi', 'nh', 'gg', 'gl', 'wow', '...'].map(r =>
          <button class="text-10px px-1.5 py-0.5 rounded border border-white/10 text-neutral-600 hover:text-zec-yellow hover:border-zec-yellow/40"
            onClick={() => { send({ type: 'Chat', text: r }); log(`you: ${r}`, 'text-white') }}>{r}</button>
        )}
      </div>
      <form class="flex border-t border-white/10" onSubmit={(e) => {
        e.preventDefault()
        const input = e.currentTarget.querySelector('input') as HTMLInputElement
        const msg = input.value.trim()
        if (!msg) return
        input.value = ''
        send({ type: 'Chat', text: msg })
        log(`you: ${msg}`, 'text-white')
      }}>
        <input class="flex-1 bg-transparent text-11px px-2 py-1.5 text-white outline-none placeholder-neutral-700" placeholder="say something…" maxLength={200} />
        <button type="submit" class="text-10px px-2 text-neutral-600 hover:text-neutral-400">↵</button>
      </form>
    </div>
  )

  return (
    <div class="h-[100dvh] flex flex-col bg-zec-dark font-sans text-white">
          {/* free chip-only tournaments — self-contained overlay, opens on #/tournaments */}
          <Tournaments />
          {/* titlebar \u2014 full-width top bar */}
          <div class="titlebar shrink-0">
            <span class="text-zec-yellow text-14px">{'\u2666'}</span>
            <span class="flex-1 text-center text-zec-yellow">zk.poker</span>
            <Show when={encrypted()}>
              <span class="text-10px text-green-400 mr-1">enc</span>
            </Show>
            <Show when={identity()}>
              <span class={`text-10px mr-1 ${identity()!.mode === 'zafu' ? 'text-zec-yellow' : 'text-neutral-500'}`}
                title={identity()!.sessionPubKey}>
                {identity()!.mode === 'zafu' ? 'zafu' : 'anon'}
              </span>
            </Show>
            <span class={`w-2 h-2 rounded-full ${connected() ? 'bg-green-500' : 'bg-neutral-600'}`} />
            <button
              class="ml-2 text-11px text-neutral-500 hover:text-zec-yellow leading-none flex items-center"
              title="tournaments"
              onClick={() => { history.pushState(null, '', '/t'); window.dispatchEvent(new PopStateEvent('popstate')) }}
            ><span class="i-lucide-trophy w-3.5 h-3.5" /></button>
            <button
              class="ml-1 text-11px text-neutral-500 hover:text-zec-yellow leading-none"
              title="relay settings"
              onClick={() => setShowSettings(true)}
            >{'⚙'}</button>
          </div>

          <Show when={showSettings()}>
            <Settings connected={connected()} onClose={() => setShowSettings(false)}
              onRename={(n) => { setName(n); send({ type: 'Rename', name: n }) }}
              pubkey={identity()?.zafuPubKey || identity()?.sessionPubKey}
              mode={identity()?.mode} />
          </Show>

          {/* main content — fills the viewport, centers non-game views */}
          <div class="flex-1 min-h-0 overflow-y-auto flex flex-col">

          {/* casino lobby */}
          <Show when={view() === 'casino'}>
            <Lobby
              hasWallet={hasWallet()}
              pubkey={walletPubkey()}
              onJoin={(table, playerName, bot) => {
                setSelectedTable(table)
                setName(playerName)
                // real-money (cash) tables create with bot=false → staked; practice-vs-bot → free-play
                setStaked(!bot)
                const params = new URLSearchParams({
                  sb: String(table.sb), bb: String(table.bb),
                  buyin: String(table.buyin), timeout: String(table.timeout),
                  rake_bps: String(table.rakeBps), rake_cap: String(table.rakeCap),
                  access: bot ? 'public' : 'private',
                  ...(bot ? { bot: 'true' } : {}),
                })
                fetch(`/new?${params}`, { redirect: 'follow' }).then(resp => {
                  const url = resp.url || resp.headers.get('location') || ''
                  const code = url.split('/').pop() || ''
                  if (code) {
                    history.pushState(null, '', '/' + code)
                    setView('lobby')
                  }
                })
              }}
              onJoinCode={(code, playerName) => {
                setName(playerName)
                history.pushState(null, '', '/' + code)
                setView('lobby')
              }}
            />
          </Show>

          {/* old lobby (for direct room links) */}
          <Show when={view() === 'lobby'}>
            <div class="p-8 text-center m-auto w-full max-w-xl">
              <div class="text-zec-yellow text-10px font-semibold uppercase tracking-3px mb-5">
                no-limit hold'em
              </div>
              {/* game parameters — only for host (creating table) */}
              <Show when={location.pathname.length <= 1} fallback={
                <Show when={isReconnect()} fallback={
                  <div class="text-neutral-500 text-11px tracking-wider mb-4">
                    joining table &middot; host sets rules
                  </div>
                }>
                  <div class="text-zec-yellow text-11px tracking-wider mb-1 uppercase">reconnecting</div>
                  <div class="text-neutral-400 text-10px mb-4">as {lastSession()!.name}</div>
                </Show>
              }>
                <div class="flex items-center justify-center gap-4 mb-3">
                  <label class="flex flex-col items-center gap-1">
                    <span class="text-neutral-500 text-11px uppercase tracking-wider">small blind</span>
                    <input class="input-field w-16 text-center" value="5" id="sb-input" />
                  </label>
                  <label class="flex flex-col items-center gap-1">
                    <span class="text-neutral-500 text-11px uppercase tracking-wider">big blind</span>
                    <input class="input-field w-16 text-center" value="10" id="bb-input" />
                  </label>
                  <label class="flex flex-col items-center gap-1">
                    <span class="text-neutral-500 text-11px uppercase tracking-wider">buy-in</span>
                    <input class="input-field w-20 text-center" value="1000" id="buyin-input" />
                  </label>
                  <label class="flex flex-col items-center gap-1">
                    <span class="text-neutral-500 text-11px uppercase tracking-wider">turn (sec)</span>
                    <input class="input-field w-16 text-center" value="30" id="timeout-input" />
                  </label>
                </div>
              </Show>
              <div class="text-neutral-600 text-11px tracking-wider mb-6">
                zk-shuffle &middot; co-signed action log &middot; encrypted
              </div>
              <div class="flex flex-col items-center gap-4">
                <div class="flex items-center justify-center gap-2">
                  <input
                    class="input-field w-48 text-center"
                    placeholder="name"
                    maxLength={16}
                    spellcheck={false}
                    value={name()}
                    onInput={e => setName(e.currentTarget.value)}
                    onKeyDown={e => { if (e.key === 'Enter') sit() }}
                    autofocus
                  />
                  <button class="btn btn-primary" onClick={sit}>
                    {isReconnect() ? 'reconnect' : location.pathname.length > 1 ? 'sit down' : 'create table'}
                  </button>
                </div>
                <Show when={hasWallet()}>
                  <div class="text-neutral-600 text-11px tracking-wide max-w-72 text-center leading-relaxed">
                    your zafu pubkey is bound to this seat &mdash; nobody else with the same name can hijack it on reconnect
                  </div>
                </Show>
              </div>
            </div>
          </Show>

          {/* waiting */}
          <Show when={view() === 'waiting'}>
            <div class="p-10 text-center relative m-auto w-full max-w-xl">
              <button
                class="absolute top-2 right-2 px-1.5 py-0.5 rounded text-10px border border-red-900 text-red-400 hover:bg-red-900/20"
                onClick={leaveTable}
                title="leave table — settles escrow and pays out"
              >leave</button>
              <div class="text-zec-yellow text-11px uppercase tracking-2px mb-4">
                {oppHere() ? 'getting ready' : 'waiting for players'}
              </div>

              {/* agreement / handshake tracker — where the two players are before the deal */}
              <AgreementTracker />

              {/* invite — a shareable link is ALWAYS available (it's just this room's URL);
                  contacts is an optional extra when the wallet exposes a picker.
                  Only show while we still need an opponent. */}
              <Show when={!oppHere()}>
              {(() => {
                const link = () => inviteUrl() || `${location.origin}/${roomCode() || currentRoom()}`
                const copy = () => { navigator.clipboard?.writeText(link()); log('copied invite link', 'c-green') }
                const sendLink = async () => {
                  if (navigator.share) {
                    try { await navigator.share({ title: 'zk.poker', text: 'join my table on zk.poker', url: link() }); return } catch { /* cancelled → fall through to copy */ }
                  }
                  copy()
                }
                const inviteContacts = async () => {
                  const contacts = await identity()?.pickContacts?.({ purpose: 'Invite to your table', max: 5 })
                  if (contacts?.length) {
                    for (const c of contacts) {
                      await identity()?.invite?.(c.handle, { type: 'poker-table-invite', data: { url: link() }, ttl: 300 })
                    }
                    log(`invited ${contacts.map(c => c.displayName).join(', ')}`, 'c-zec-yellow')
                  }
                }
                return (
                  <div class="mb-5">
                    <div class="text-neutral-500 text-11px uppercase tracking-wider mb-2">invite a player</div>
                    <div
                      class="input-field text-11px text-center cursor-pointer select-all mb-2 break-all"
                      onClick={copy}
                      title="click to copy"
                    >{link()}</div>
                    <div class="flex gap-2 justify-center flex-wrap">
                      <button class="btn btn-primary text-11px px-4 py-1.5" onClick={sendLink}>send link</button>
                      <button class="btn text-11px px-4 py-1.5" onClick={copy}>copy</button>
                      <Show when={identity()?.pickContacts}>
                        <button class="btn text-11px px-4 py-1.5" onClick={inviteContacts}>contacts</button>
                      </Show>
                    </div>
                  </div>
                )
              })()}
              </Show>

              {/* stake agreement action — accept the opponent's proposed stakes.
                  This is the live action for the "agree on stakes" step above. */}
              <Show when={pendingRules() && !pendingRules()?.fromSelf}>
                <div class="mt-1 p-4 border border-zec-yellow/30 bg-zec-yellow/5 rounded max-w-sm mx-auto">
                  <div class="text-neutral-400 text-10px uppercase tracking-wider mb-2">opponent proposes stakes</div>
                  <div class="text-white text-12px font-mono mb-3">
                    {pendingRules()!.smallBlind}/{pendingRules()!.bigBlind} blinds · {pendingRules()!.buyin} buyin
                  </div>
                  <button class="btn btn-primary text-11px px-6" onClick={() => send({ type: 'AcceptRules' })}>
                    accept stakes
                  </button>
                </div>
              </Show>
              <Show when={pendingRules()?.fromSelf}>
                <div class="text-neutral-500 text-11px max-w-sm mx-auto">
                  waiting for opponent to accept your stakes…
                </div>
              </Show>

              {/* stuck-state escape — the handshake auto-retries first (ws.ts); if we're still
                  here after the grace period the opponent likely dropped or walked away. Never
                  leave anyone permanently stuck: offer a clear, safe way out (no funds are in
                  escrow yet at this stage). */}
              <Show when={handshakeStuck()}>
                <div class="mt-4 p-3 border border-red-500/40 bg-red-500/5 rounded max-w-sm mx-auto text-left">
                  <div class="text-red-300 text-12px font-semibold mb-1">taking longer than expected</div>
                  <div class="text-neutral-400 text-11px leading-snug mb-3">
                    your opponent hasn't confirmed — they may have stepped away or lost connection.
                    nothing is at stake yet.
                  </div>
                  <div class="flex gap-2">
                    <button class="btn btn-primary text-11px px-4 py-1.5" onClick={leaveTable}>return to lobby</button>
                    <button class="btn text-11px px-4 py-1.5" onClick={() => setHandshakeStuck(false)}>keep waiting</button>
                  </div>
                </div>
              </Show>

              {/* once both are seated, let them talk while the table sets up */}
              <Show when={oppHere()}>
                <TableChat />
              </Show>
            </div>
          </Show>

          {/* deposit */}
          <Show when={view() === 'deposit'}>
            <div class="p-6 relative m-auto w-full max-w-xl">
              <button
                class="absolute top-2 right-2 px-1.5 py-0.5 rounded text-10px border border-red-900 text-red-400 hover:bg-red-900/20"
                onClick={leaveTable}
                title="leave table — refund both deposits"
              >leave</button>
              <div class="text-zec-yellow text-11px uppercase tracking-2px mb-1 text-center">deposit to play</div>
              <div class="text-neutral-500 text-11px text-center mb-4">2-of-3 multisig escrow (you + opp + house)</div>

              {/* same agreement tracker as the waiting screen — continuity through the handshake */}
              <AgreementTracker />

              {(() => {
                const seat = mySeat()
                const myAddr = seatAddresses()[seat] ?? null
                const myDep = seat === 0 ? depositA() : depositB()
                const oppDep = seat === 0 ? depositB() : depositA()
                const req = requiredDeposit()
                const myReady = myDep >= req
                const oppReady = oppDep >= req
                // 0-conf: tx visible in the mempool but not yet mined. Deal already starts on
                // this (server gate = confirmed+pending); show it so the ~75s block wait feels instant.
                const myPend = seat === 0 ? pendingA() : pendingB()
                const oppPend = seat === 0 ? pendingB() : pendingA()
                const mySeen = !myReady && (myDep + myPend) >= req
                const oppSeen = !oppReady && (oppDep + oppPend) >= req
                const reqZec = (req / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')
                const myZec = (myDep / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')
                const oppZec = (oppDep / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')
                return <>
                  <div class="mb-4 p-3 border border-white/10 rounded-lg bg-zec-surface">
                    <div class="text-neutral-500 text-11px uppercase tracking-wider mb-2">your deposit address</div>
                    <Show when={myAddr} fallback={
                      <div class="text-neutral-600 text-11px">waiting for multisig setup…</div>
                    }>
                      <div
                        class="font-mono text-11px text-zec-yellow break-all cursor-pointer select-all"
                        onClick={() => { navigator.clipboard?.writeText(myAddr!); log('copied deposit address', 'c-green') }}
                        title="click to copy"
                      >{myAddr}</div>
                      <div class="mt-2 flex items-center gap-2">
                        <span class="text-neutral-500 text-11px uppercase tracking-wider">send</span>
                        <span class="text-zec-yellow text-11px tabular" title="buy-in + your share of the on-chain payout fee">{reqZec} ZEC</span>
                      </div>
                      {depositFeePerSeat() > 0 && (
                        <div class="mt-1 text-neutral-500 text-11px tabular">
                          = {(depositBuyinZat() / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')} buy-in
                          {' + '}
                          {(depositFeePerSeat() / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')} payout fee
                        </div>
                      )}
                      <div class="mt-3 pt-2 border-t border-white/10">
                        <div class="text-neutral-500 text-11px uppercase tracking-wider mb-1">payouts will go to</div>
                        <input
                          class="w-full bg-zec-bg border border-white/15 rounded px-2 py-1 font-mono text-10px text-zec-yellow"
                          placeholder="u1... (your orchard UA)"
                          value={payoutOverride() ?? ''}
                          onInput={(e) => {
                            const v = e.currentTarget.value.trim()
                            if (v === '') setPayoutOverride(null)
                            else if (v.startsWith('u1') || v.startsWith('utest1') || v.startsWith('uregtest1')) setPayoutOverride(v)
                            else setPayoutOverride(null)
                          }}
                        />
                        <div class="mt-1 text-neutral-600 text-11px">refunds and winnings land here. paste your zafu / wallet orchard UA.</div>
                      </div>
                      <button
                        class="mt-3 w-full btn btn-primary text-12px py-2.5 disabled:opacity-40 disabled:cursor-not-allowed"
                        disabled={!payoutOverride() || (sendTriggered() && !myReady)}
                        onClick={() => {
                          try {
                            // FUND-SAFETY: never dispatch a second send while the first is still
                            // unconfirmed — a repeat click would over-deposit real ZEC.
                            if (sendTriggered() && !myReady) { log('deposit already sent — waiting for confirmation', 'c-zec-yellow'); return }
                            const addr = payoutOverride()
                            if (!addr) { log('paste a payout address first', 'c-red'); return }
                            const providers = (window as any)[Symbol.for('penumbra')]
                            const extId = providers ? (Object.keys(providers)[0]?.replace('chrome-extension://','').replace(/\/$/, '')) : null
                            if (!extId) { log('zafu not detected', 'c-red'); return }
                            // pin the deposit to this player's ed25519 session identity (64-hex pubkey)
                            // so the escrow scanner can bind the on-chain deposit to the seat that made it.
                            const sessionPub = identity()?.sessionPubKey ?? ''
                            const memo = /^[0-9a-f]{64}$/.test(sessionPub)
                              ? `zk.poker/v1/payout:${addr};id:${sessionPub}`
                              : `zk.poker/v1/payout:${addr}`
                            chrome.runtime.sendMessage(extId, {
                              type: 'send',
                              address: myAddr,
                              amount_zat: req,
                              memo,
                            }, () => {})
                            setSendTriggered(true)
                            markDepositSent() // persist so a reload can't re-arm Send before the tx confirms
                            // deposit-fault detector: a deposit confirms in ~1-2 blocks (~75s).
                            // if it's still uncredited after 4 min, the scan may have missed it
                            // (wrong address, missing memo, scanner lag) — exactly the failure
                            // that stranded the earlier test deposit. Report it to the escrow
                            // journal so it surfaces instead of silently hanging.
                            setTimeout(() => {
                              const dep = seat === 0 ? depositA() : depositB()
                              if (dep < requiredDeposit()) {
                                send({ type: 'EscrowFault', phase: 'deposit', detail: `deposit not credited after 4m: sent ${req} zat to ${myAddr}` })
                                log('deposit not detected yet — flagged to escrow for review', 'c-zec-yellow')
                              }
                            }, 240000)
                          } catch (e: any) { log(`zafu send failed: ${e?.message ?? e}`, 'c-red') }
                        }}
                      >{sendTriggered() && !myReady ? 'Deposit sent — confirming…' : `Send ${reqZec} ZEC with zafu`}</button>
                      <Show when={!payoutOverride()}>
                        <div class="mt-1.5 text-zec-yellow/70 text-11px text-center">enter your payout address above to continue</div>
                      </Show>
                      {/* need ZEC? point at the exchanges that list it */}
                      <div class="mt-3 pt-3 border-t border-white/10">
                        <div class="text-neutral-500 text-11px mb-1.5">need ZEC? buy it and send to your wallet:</div>
                        <div class="flex flex-wrap gap-1.5">
                          {[
                            ['Binance', 'https://www.binance.com/en/trade/ZEC_USDT'],
                            ['Kraken', 'https://www.kraken.com/prices/zcash'],
                            ['Coinbase', 'https://www.coinbase.com/price/zcash'],
                            ['Hyperliquid', 'https://app.hyperliquid.xyz/trade/ZEC'],
                            ['NEAR Intents', 'https://near-intents.org/'],
                          ].map(([label, url]) => (
                            <a href={url} target="_blank" rel="noopener"
                              class="text-11px px-2 py-1 rounded bg-zec-surface border border-white/10 text-zec-text hover:border-zec-yellow/50 no-underline">{label}</a>
                          ))}
                        </div>
                        <div class="mt-1.5 text-neutral-600 text-11px leading-relaxed">
                          exchanges send transparent <span class="font-mono">t1…</span> ZEC — tap <span class="text-white/60">shield</span> in zafu once to get your private <span class="font-mono">u1…</span> address.
                        </div>
                      </div>
                      <div class="mt-2 text-neutral-600 text-11px leading-relaxed">
                        sending from an external wallet? attach memo:{' '}
                        <span class="font-mono text-neutral-500">zk.poker/v1/payout:&lt;your-u1-address&gt;</span>
                      </div>
                    </Show>
                  </div>

                  <div class="grid grid-cols-2 gap-3 mb-4">
                    <div class="p-2 border border-white/10 rounded bg-zec-surface">
                      <div class="text-neutral-500 text-10px uppercase">you</div>
                      <div class="tabular text-11px mt-1">
                        <span class={myReady ? 'c-green' : 'c-zec-yellow'}>{myZec}</span>
                        <span class="text-neutral-600"> / {reqZec} ZEC</span>
                      </div>
                      <div class={`text-11px mt-1 flex items-center gap-1 ${myReady ? 'c-green' : mySeen ? 'c-zec-yellow' : 'text-neutral-500'}`}>
                        <Show when={myReady} fallback={
                          <Show when={mySeen} fallback={<><div class="i-lucide-loader-2 animate-spin h-3 w-3" /><span>waiting for tx in block</span></>}>
                            <div class="i-lucide-loader-2 animate-spin h-3 w-3" /><span>seen in mempool — dealing…</span>
                          </Show>
                        }>
                          <span>✓ deposited</span>
                        </Show>
                      </div>
                    </div>
                    <div class="p-2 border border-white/10 rounded bg-zec-surface">
                      <div class="text-neutral-500 text-10px uppercase">opponent</div>
                      <div class="tabular text-11px mt-1">
                        <span class={oppReady ? 'c-green' : 'c-zec-yellow'}>{oppZec}</span>
                        <span class="text-neutral-600"> / {reqZec} ZEC</span>
                      </div>
                      <div class={`text-11px mt-1 flex items-center gap-1 ${oppReady ? 'c-green' : oppSeen ? 'c-zec-yellow' : 'text-neutral-500'}`}>
                        <Show when={oppReady} fallback={
                          <Show when={oppSeen} fallback={<><div class="i-lucide-loader-2 animate-spin h-3 w-3" /><span>waiting for tx in block</span></>}>
                            <div class="i-lucide-loader-2 animate-spin h-3 w-3" /><span>seen in mempool — dealing…</span>
                          </Show>
                        }>
                          <span>✓ deposited</span>
                        </Show>
                      </div>
                    </div>
                  </div>

                  <Show when={sendTriggered() && !myReady}>
                    <div class="text-center text-zec-yellow/80 text-11px mb-2">
                      tx sent — confirms in 1-2 blocks (~75s). don't send again.
                    </div>
                  </Show>
                  <div class="text-center text-neutral-500 text-11px">
                    table starts when both players have deposited
                  </div>
                </>
              })()}
              <Show when={oppHere()}>
                <TableChat />
              </Show>
            </div>
          </Show>

          {/* game */}
          <Show when={view() === 'game'}>
            <div class="flex-1 min-h-0 w-full max-w-screen-2xl mx-auto px-2 lg:px-8 lg:py-3 lg:flex lg:gap-6">
             <div class="lg:flex-1 lg:flex lg:flex-col lg:min-h-0">
              {/* status bar */}
              <div class="flex justify-between items-center px-2 py-2 text-11px text-white/50 uppercase tracking-wider">
                <span>
                  hand #{handNum()}
                  <Show when={deckVerified()}>
                    <span class="text-green-400 ml-1" title="deck verified via Chaum-Pedersen proof">✓</span>
                  </Show>
                </span>
                <Show when={juryProgress()}>
                  <span class="text-zec-yellow animate-pulse">{juryProgress()}</span>
                </Show>
                <span class="flex items-center gap-2">
                  <span>you: {getPositionShort(mySeat(), button(), maxSeats())}</span>
                  <button
                    class={`px-1.5 py-0.5 rounded text-10px border ${broadcasting() ? 'border-red-500 text-red-400 bg-red-900/20' : 'border-white/15 text-neutral-600 hover:text-neutral-400'}`}
                    onClick={() => setBroadcasting(b => !b)}
                    title={broadcasting() ? 'stop broadcasting to spectators' : 'broadcast game to spectators (public info only)'}
                  >{broadcasting() ? 'LIVE' : 'broadcast'}</button>
                  <Show when={broadcasting()}>
                    <button
                      class={`px-1.5 py-0.5 rounded text-10px border ${showMyCards() ? 'border-zec-yellow text-zec-yellow' : 'border-white/15 text-neutral-600'}`}
                      onClick={() => setShowMyCards(s => !s)}
                      title="toggle showing your hole cards to spectators"
                    >{showMyCards() ? 'cards: shown' : 'cards: hidden'}</button>
                  </Show>
                  <button
                    class="px-1.5 py-0.5 rounded text-10px border border-red-900 text-red-400 hover:bg-red-900/20"
                    onClick={leaveTable}
                    title="leave table — settles escrow and pays out"
                  >leave</button>
                </span>
              </div>

              {/* felt — charcoal stadium, thin rail + accent keyline, grows to fill viewport */}
              <div class="rounded-999px px-2 sm:px-5 py-4 sm:py-6 lg:py-10 relative flex items-center justify-center lg:flex-1 lg:min-h-0"
                style="min-height: 240px;
                  background: radial-gradient(ellipse at 50% 38%, #2b2f36 0%, #1e2126 55%, #14161a 100%);
                  border: 7px solid #101216;
                  box-shadow: inset 0 0 0 1px rgba(244,183,40,0.28), inset 0 0 60px 12px rgba(0,0,0,0.5), inset 0 2px 3px rgba(255,255,255,0.05), 0 16px 48px rgba(0,0,0,0.5)">

                {/* brand watermark */}
                <div class="absolute inset-0 flex items-center justify-center pointer-events-none select-none" aria-hidden="true">
                  <span class="text-100px lg:text-180px leading-none" style="color: rgba(244,183,40,0.045)">♦</span>
                </div>

                {/* disconnect overlay */}
                <Show when={oppDisconnected()}>
                  <div class="absolute inset-0 bg-black/60 z-10 flex items-center justify-center rounded-inherit">
                    <div class="text-center">
                      <div class="text-red-400 text-11px uppercase tracking-wider mb-1">opponent disconnected</div>
                      <div class="font-mono text-18px text-white">{reconnectCountdown()}s</div>
                      <div class="text-neutral-500 text-11px">waiting for reconnect</div>
                    </div>
                  </div>
                </Show>

                {/* shuffle/status overlay */}
                <Show when={gameStatus() && !gameStatus().includes('verified') && acting() < 0}>
                  <div class="absolute inset-0 bg-black/40 z-10 flex items-center justify-center rounded-inherit">
                    <div class="text-center">
                      <div class="text-zec-yellow text-11px uppercase tracking-wider mb-2 animate-pulse">{gameStatus()}</div>
                      <div class="flex items-end justify-center gap-1 h-4">
                        {[0,.08,.16,.24,.32].map(d =>
                          <div class="w-1 rounded-sm bg-zec-yellow animate-pulse" style={`animation-delay:${d}s; height: 60%`} />
                        )}
                      </div>
                    </div>
                  </div>
                </Show>

                {/* opponent (top) */}
                <div class="absolute top--4 left-50% -translate-x-50% text-center">
                  <div class="flex items-center justify-center gap-2">
                    <div class="w-20 flex justify-end">
                      <Show when={oppBet() > 0}>
                        <div class="felt-pill text-12px font-500 text-white/87">{oppBet()}</div>
                      </Show>
                    </div>
                    <div class={`inline-block px-4 py-1.5 min-w-28 pod ${acting() === opp() ? 'seat-acting' : ''} ${oppDisconnected() ? 'border-red-900! opacity-70' : ''}`}>
                      <div class={`text-12px font-medium ${acting() === opp() ? 'text-zec-text' : oppDisconnected() ? 'text-red-400' : 'text-white/60'}`}
                        title={oppPubkey()
                          ? `identity: ${oppPubkey()}\n${oppVerified() ? 'verified ✓ — delegation binds this key to the encrypted session' : oppVerified() === false ? 'UNVERIFIED — delegation did not check out' : 'verifying…'}`
                          : 'anonymous session — opponent has no wallet identity'}>
                        {oppName()} <span class="text-white/38 text-10px uppercase">{getPositionShort(opp(), button(), maxSeats())}</span>
                        <Show when={oppPubkey()}>
                          <span class={`ml-0.5 text-10px cursor-help ${oppVerified() ? 'text-green-400' : oppVerified() === false ? 'text-red-400' : 'text-white/40'}`}
                            title={`identity ${oppPubkey()}`}>{oppVerified() ? '✓' : oppVerified() === false ? '⚠' : '…'}</span>
                        </Show>
                        {oppDisconnected() ? ' (dc)' : ''}
                      </div>
                      <div class="font-mono tabular-nums text-16px lg:text-18px font-600 text-white/87">{oppStack()}</div>
                      <Show when={acting() === opp()}>
                        <TimerBar />
                      </Show>
                    </div>
                    <div class="w-20" aria-hidden="true"></div>
                  </div>
                  <div class="flex gap-1 justify-center mt-1.5">
                    <Show when={oppRevealed() && oppCards()} fallback={
                      <Show when={myCards()}>
                        <Card /><Card />
                      </Show>
                    }>
                      <Card card={oppCards()![0]} />
                      <Card card={oppCards()![1]} />
                    </Show>
                  </div>
                </div>

                {/* dealer chip — accent disc, travels between seats */}
                <Show when={button() === mySeat()}>
                  <div class="absolute bottom-12 rounded-full w-6 h-6 bg-zec-yellow text-black text-10px font-bold leading-6 text-center z-5 transition-all duration-400"
                    style="left: calc(50% + 62px); box-shadow: 0 2px 5px rgba(0,0,0,0.5), inset 0 1px 1px rgba(255,255,255,0.55)">D</div>
                </Show>
                <Show when={button() === opp()}>
                  <div class="absolute top-12 rounded-full w-6 h-6 bg-zec-yellow text-black text-10px font-bold leading-6 text-center z-5 transition-all duration-400"
                    style="left: calc(50% + 62px); box-shadow: 0 2px 5px rgba(0,0,0,0.5), inset 0 1px 1px rgba(255,255,255,0.55)">D</div>
                </Show>

                {/* board + pot — cards centered, pot pill directly below (GG convention) */}
                <div class="flex flex-col items-center gap-2.5 my-10 lg:my-0 relative z-1">
                <div class="flex gap-1.5 sm:gap-2 justify-center items-center">
                  {/* deck on table — shows shuffle status */}
                  <Show when={board().length === 0}>
                    <div class="relative w-11 h-15.5 sm:w-12 sm:h-17" title={deckVerified() ? 'deck verified (Chaum-Pedersen)' : gameStatus() || 'deck'}>
                      {/* stacked card backs */}
                      <div class="absolute inset-0 rounded-sm border border-white/15 bg-zec-surface"
                        style="transform: rotate(-2deg); background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)" />
                      <div class="absolute inset-0 rounded-sm border border-white/15 bg-zec-surface"
                        style="transform: rotate(1deg); background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)" />
                      <div class={`absolute inset-0 rounded-sm border bg-zec-surface flex items-center justify-center text-11px font-bold ${
                        gameStatus().includes('shuffl') || gameStatus().includes('key') ? 'border-zec-yellow animate-pulse text-zec-yellow' :
                        deckVerified() ? 'border-green-500 text-green-400' :
                        'border-white/15 text-neutral-600'
                      }`} style="background-image: repeating-linear-gradient(45deg,transparent,transparent 3px,rgba(244,183,40,0.05) 3px,rgba(244,183,40,0.05) 4px)">
                        {gameStatus().includes('shuffl') || gameStatus().includes('key') || gameStatus().includes('prov') ? '...' :
                         deckVerified() ? '✓' : '52'}
                      </div>
                    </div>
                  </Show>
                  <For each={board()}>
                    {(c, i) => <Card card={c} size="lg" dealIndex={board().length <= 3 ? i() : 0} />}
                  </For>
                </div>

                {/* pot pill */}
                <div class="felt-pill text-13px lg:text-15px font-600 text-white/87">
                  <Show when={lastResult()} fallback={<><span class="text-white/50 font-sans font-500 mr-1.5">pot</span>{pot()}</>}>
                    <span class={`animate-pulse ${lastResult()!.won ? 'text-green-400' : 'text-red-400'}`}>
                      {lastResult()!.won ? '+' : ''}{lastResult()!.amount}
                    </span>
                  </Show>
                </div>
                </div>

                {/* you (bottom) */}
                <div class="absolute bottom--4 left-50% -translate-x-50% text-center">
                  <div class="flex gap-1 justify-center mb-1.5">
                    <Show when={myCards()}>
                      <Card card={myCards()![0]} dealIndex={0} />
                      <Card card={myCards()![1]} dealIndex={1} />
                    </Show>
                  </div>
                  <div class="flex items-center justify-center gap-2">
                    <div class="w-20 flex justify-end">
                      <Show when={myBet() > 0}>
                        <div class="felt-pill text-12px font-500 text-white/87">{myBet()}</div>
                      </Show>
                    </div>
                    <div class={`inline-block px-4 py-1.5 min-w-28 pod ${acting() === mySeat() ? 'seat-acting' : ''}`}>
                      <Show when={acting() === mySeat()}>
                        <TimerBar />
                      </Show>
                      <div class="font-mono tabular-nums text-16px lg:text-18px font-600 text-zec-text">{myStack()}</div>
                      <div class={`text-12px font-medium ${acting() === mySeat() ? 'text-zec-text' : 'text-white/60'}`}>
                        {name() || 'you'} <span class="text-white/38 text-10px uppercase">{button() === mySeat() ? 'BTN/SB' : 'BB'}</span>
                      </div>
                    </div>
                    <div class="w-20" aria-hidden="true"></div>
                  </div>
                </div>
              </div>

              {/* actions */}
              <div class="flex gap-1 sm:gap-1.5 justify-center items-center py-2 sm:py-3 min-h-11 flex-wrap">
                <Show when={isMyTurn() && actions().length > 0} fallback={
                  <Show when={acting() >= 0 && !isMyTurn()}>
                    <span class="text-neutral-600 text-10px uppercase tracking-wider">
                      {oppDisconnected() ? 'hand paused — opponent offline' : 'opponent to act'}
                    </span>
                  </Show>
                }>
                  {/* sizing buttons — Pluribus-style: 1/4, 1/2, 3/4, pot, 2x */}
                  {(() => {
                    const betAction = actions().find(v => v.kind === 'raise') || actions().find(v => v.kind === 'bet')
                    if (!betAction) return null
                    const p = pot()
                    const min = betAction.min_amount || 0
                    const max = betAction.max_amount || 0
                    const clamp = (v: number) => Math.min(Math.max(Math.round(v), min), max)
                    const sizes = [
                      { label: '¼', val: clamp(p / 4) },
                      { label: '½', val: clamp(p / 2) },
                      { label: '¾', val: clamp(p * 3 / 4) },
                      { label: 'pot', val: clamp(p) },
                      { label: '2x', val: clamp(p * 2) },
                    ].filter(s => s.val >= min && s.val <= max)
                    // dedupe sizes that collapse to same value
                    const unique = sizes.filter((s, i) => i === 0 || s.val !== sizes[i-1].val)
                    return <div class="w-full flex gap-1 justify-center mb-1.5">
                      {unique.map(s =>
                        <button class={`chip ${raiseVal() === s.val ? 'chip-active' : ''}`}
                          onClick={() => setRaiseVal(s.val)}>{s.label}</button>
                      )}
                    </div>
                  })()}
                  {/* main action buttons — fold → check/call → bet/raise → all-in */}
                  <For each={sortedActions()}>
                    {a => {
                      const two = (verb: string, amt?: number | string) => <>
                        <span class="block leading-tight">{verb}</span>
                        {amt !== undefined && <span class="block leading-tight text-11px opacity-75 font-mono tabular-nums">{amt}</span>}
                      </>
                      if (a.kind === 'fold')
                        return <button class="btn btn-danger min-h-12 sm:min-h-11 min-w-20 px-4" onClick={() => act('fold')}>{two('fold')}</button>
                      if (a.kind === 'check')
                        return <button class="btn min-h-12 sm:min-h-11 min-w-20 px-4" onClick={() => act('check')}>{two('check')}</button>
                      if (a.kind === 'call')
                        return <button class="btn btn-call min-h-12 sm:min-h-11 min-w-20 px-4" onClick={() => act('call')}>{two('call', a.min_amount)}</button>
                      if (a.kind === 'bet')
                        return <div class="flex items-center gap-1.5">
                          <input class="input-field w-16 sm:w-20 text-center" type="number"
                            min={a.min_amount} max={a.max_amount}
                            value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                          <button class="btn btn-primary min-h-12 sm:min-h-11 min-w-20 px-4" onClick={() => act('bet', raiseVal())}>{two('bet', raiseVal())}</button>
                        </div>
                      if (a.kind === 'raise')
                        return <div class="flex items-center gap-1.5">
                          <Show when={!actions().some(x => x.kind === 'bet')}>
                            <input class="input-field w-16 sm:w-20 text-center" type="number"
                              min={a.min_amount} max={a.max_amount}
                              value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                          </Show>
                          <button class="btn btn-primary min-h-12 sm:min-h-11 min-w-20 px-4" onClick={() => act('raise', raiseVal())}>{two('raise to', raiseVal())}</button>
                        </div>
                      if (a.kind === 'allin')
                        return <button class="btn btn-allin min-h-12 sm:min-h-11 min-w-20 px-4" onClick={() => act('allin')}>{two('all in')}</button>
                      return null
                    }}
                  </For>
                </Show>
              </div>

              {/* auto-action presets — always visible */}
              <div class="flex gap-1.5 justify-center py-2">
                {(['check/fold', 'check', 'fold', 'call any'] as const).map(mode =>
                  <button
                    class={`chip ${autoAction() === mode ? 'chip-active' : ''}`}
                    onClick={() => setAutoAction(autoAction() === mode ? 'none' : mode)}
                  >
                    {mode}
                  </button>
                )}
              </div>

              {/* hotkey legend + mode toggle */}
              <div class="flex items-center justify-center gap-2 py-0.5">
                <button
                  class={`text-10px px-1.5 py-0.5 rounded border ${keyMode() === 'classic' ? 'border-zec-yellow text-zec-yellow' : 'border-white/10 text-neutral-700'}`}
                  onClick={() => setKeyMode('classic')}
                  title="PokerStars-style hotkeys"
                >classic</button>
                <button
                  class={`text-10px px-1.5 py-0.5 rounded border ${keyMode() === 'vim' ? 'border-zec-yellow text-zec-yellow' : 'border-white/10 text-neutral-700'}`}
                  onClick={() => setKeyMode('vim')}
                  title="vim-style hotkeys"
                >vim</button>
              </div>
              <div class="text-center text-11px text-neutral-700 pb-0.5">
                <Show when={keyMode() === 'classic'}>
                  F1 fold · F2 check/call · F3 raise · F4 pot · Space call · Q all-in · 1-5 sizing
                </Show>
                <Show when={keyMode() === 'vim'}>
                  f fold · d check · s call · r/w raise · a all-in · j/k size ±  · H ½p · M ¾p · G pot · L 2x
                </Show>
              </div>

              {/* media controls + video — opt-in direct P2P (leaks IP to peer) */}
              <div class="flex items-center justify-between px-1 py-1">
                <div class="flex items-center gap-1">
                  <button
                    class={`text-11px px-2 py-0.5 rounded border ${media()?.micEnabled() ? 'border-green-500 text-green-400' : 'border-white/15 text-neutral-600 hover:text-neutral-400'}`}
                    onClick={() => requestMedia('mic')}
                    title="Voice chat connects you directly to your opponent and reveals your IP address."
                  >{media()?.micEnabled() ? 'mic on' : 'mic'}</button>
                  <button
                    class={`text-11px px-2 py-0.5 rounded border ${media()?.camEnabled() ? 'border-green-500 text-green-400' : 'border-white/15 text-neutral-600 hover:text-neutral-400'}`}
                    onClick={() => requestMedia('cam')}
                    title="Video chat connects you directly to your opponent and reveals your IP address."
                  >{media()?.camEnabled() ? 'cam on' : 'cam'}</button>
                  <Show when={media()?.camEnabled()}>
                    <button
                      class={`text-11px px-2 py-0.5 rounded border ${media()?.blurMode() !== 'off' ? 'border-green-500 text-green-400' : 'border-white/15 text-neutral-600 hover:text-neutral-400'}`}
                      onClick={() => media()?.setBlurMode(media()?.blurMode() === 'off' ? 'blur' : 'off')}
                      title="Blur your webcam background (processed locally, only the blurred video is sent to your opponent)."
                    >{media()?.blurUnavailable() ? 'blur n/a' : media()?.blurMode() === 'blur' ? 'blur on' : 'blur'}</button>
                  </Show>
                  <Show when={media()?.acknowledged()}>
                    <button
                      class="text-11px px-2 py-0.5 rounded border border-white/15 text-neutral-600 hover:text-red-400 hover:border-red-500/50"
                      onClick={() => media()?.revoke()}
                      title="Stop voice/video and close the direct connection."
                    >disconnect</button>
                  </Show>
                </div>
              </div>

              {/* Floating video tiles — free to DRAG anywhere and RESIZE (drag the
                  bottom-right corner). Rendered position:fixed so they float over
                  the table; positions persist for the session. */}
              <Show when={media()?.remoteStream()}>
                {/* REMOTE tile — NOT muted (we want to hear + see the opponent). */}
                <div
                  data-tile="remote"
                  class="fixed z-40 overflow-hidden rounded border border-white/20 bg-black/80 shadow-lg cursor-move"
                  style={{
                    left: remotePos() ? `${remotePos()!.x}px` : 'auto',
                    top: remotePos() ? `${remotePos()!.y}px` : '80px',
                    right: remotePos() ? 'auto' : '16px',
                    width: '176px',
                    height: '132px',
                    resize: 'both',
                    'min-width': '96px',
                    'min-height': '72px',
                  }}
                  ref={el => makeDraggable(el, setRemotePos)}
                >
                  <video
                    class="w-full h-full object-cover pointer-events-none"
                    ref={el => bindVideo(el, () => media()?.remoteStream() ?? null, {
                      muted: false,
                      onBlocked: setRemoteNeedsTap,
                    })}
                  />
                  <span class="absolute top-0 left-0 px-1 text-9px text-white/60 bg-black/40 rounded-br pointer-events-none">opponent</span>
                  <Show when={remoteNeedsTap()}>
                    <button
                      data-nodrag
                      class="absolute inset-0 flex items-center justify-center bg-black/60 text-10px text-white"
                      title="Autoplay was blocked — tap to start the opponent's video/audio."
                      onClick={(e) => {
                        e.stopPropagation()
                        const v = (e.currentTarget.parentElement?.querySelector('video') as HTMLVideoElement | null)
                        v?.play().then(() => setRemoteNeedsTap(false)).catch(() => {})
                      }}
                    >tap to play</button>
                  </Show>
                </div>
              </Show>
              <Show when={media()?.localStream() && media()?.camEnabled()}>
                {/* LOCAL preview — MUTED to avoid hearing your own mic (echo). */}
                <div
                  data-tile="local"
                  class="fixed z-40 overflow-hidden rounded border border-white/15 bg-black/80 shadow-lg cursor-move opacity-80 hover:opacity-100"
                  style={{
                    left: localPos() ? `${localPos()!.x}px` : 'auto',
                    top: localPos() ? `${localPos()!.y}px` : '220px',
                    right: localPos() ? 'auto' : '16px',
                    width: '128px',
                    height: '96px',
                    resize: 'both',
                    'min-width': '80px',
                    'min-height': '60px',
                  }}
                  ref={el => makeDraggable(el, setLocalPos)}
                >
                  <video
                    class="w-full h-full object-cover pointer-events-none"
                    ref={el => bindVideo(el, () => media()?.localStream() ?? null, { muted: true })}
                  />
                  <span class="absolute top-0 left-0 px-1 text-9px text-white/60 bg-black/40 rounded-br pointer-events-none">you</span>
                </div>
              </Show>

              {/* media error + retry — every failure mode (permission denied,
                  device busy, no device, negotiation/ICE failure) lands here with
                  a clear message and a Retry that re-runs just the failed step, so
                  the user is never stuck without a page reload. */}
              <Show when={media()?.lastError()}>
                <div class="mx-1 mb-1 px-2 py-1.5 rounded border border-red-500/40 bg-red-500/5 flex items-center justify-between gap-2">
                  <span class="text-11px text-red-300 leading-tight">{media()?.lastError()?.message}</span>
                  <div class="flex gap-1 flex-shrink-0">
                    <button
                      class="text-11px px-2 py-0.5 rounded border border-white/15 text-neutral-500 hover:text-neutral-300"
                      onClick={() => media()?.clearError()}
                    >Dismiss</button>
                    <button
                      class="text-11px px-2 py-0.5 rounded border border-green-500 text-green-400 hover:bg-green-500/10"
                      onClick={() => media()?.retry()}
                    >Retry</button>
                  </div>
                </div>
              </Show>

              {/* incoming media: opponent started video/voice but we haven't opted in.
                  We never auto-connect (no PC, no ICE, no IP leak) — we prompt, and
                  accepting routes through the same acknowledgement dialog. */}
              <Show when={media()?.incomingPending() && !media()?.acknowledged()}>
                <div class="mx-1 mb-1 px-2 py-1.5 rounded border border-zec-yellow/40 bg-zec-yellow/5 flex items-center justify-between gap-2">
                  <span class="text-11px text-neutral-300 leading-tight">
                    Opponent enabled voice/video — enable yours to connect.
                    <span class="text-neutral-500"> This reveals your IP to them, and theirs to you.</span>
                  </span>
                  <div class="flex gap-1 flex-shrink-0">
                    <button
                      class="text-11px px-2 py-0.5 rounded border border-white/15 text-neutral-500 hover:text-neutral-300"
                      onClick={() => media()?.dismissIncoming()}
                    >Ignore</button>
                    <button
                      class="text-11px px-2 py-0.5 rounded border border-green-500 text-green-400 hover:bg-green-500/10"
                      onClick={acceptIncomingMedia}
                    >Enable</button>
                  </div>
                </div>
              </Show>

              {/* IP-exposure acknowledgement — gates the first getUserMedia / RTCPeerConnection */}
              <Show when={showMediaAck()}>
                <div
                  class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
                  onClick={cancelMediaAck}
                >
                  <div
                    class="max-w-sm w-full bg-zec-surface border border-white/15 rounded-lg p-4 text-left"
                    onClick={(e) => e.stopPropagation()}
                  >
                    <div class="text-sm font-semibold text-white mb-2">Enable voice/video?</div>
                    <p class="text-11px text-neutral-400 leading-relaxed mb-2">
                      Voice and video connect you <span class="text-white">directly</span> to your
                      opponent so audio and video never pass through our servers. Because it is a
                      direct connection, <span class="text-zec-yellow">your IP address becomes visible
                      to your opponent, and theirs to you</span>.
                    </p>
                    <p class="text-11px text-neutral-500 leading-relaxed mb-3">
                      Your poker game stays private and relayed either way — this only affects the
                      optional voice/video chat. You can turn it back off at any time.
                    </p>
                    <div class="flex justify-end gap-2">
                      <button
                        class="text-11px px-3 py-1 rounded border border-white/15 text-neutral-400 hover:text-neutral-200"
                        onClick={cancelMediaAck}
                      >Cancel</button>
                      <button
                        class="text-11px px-3 py-1 rounded border border-green-500 text-green-400 hover:bg-green-500/10"
                        onClick={confirmMediaAck}
                      >I understand — enable</button>
                    </div>
                  </div>
                </div>
              </Show>

             </div>{/* end main column */}

              {/* log + chat sidebar — full-height rail on desktop */}
              <div class="lg:w-80 lg:flex-shrink-0 lg:flex lg:flex-col lg:min-h-0 lg:py-1">
              <div ref={logEl!} class="bg-zec-surface border border-white/10 rounded-md p-2 max-h-28 lg:max-h-none lg:flex-1 lg:min-h-0 overflow-y-auto font-mono text-10px mb-1 leading-relaxed">
                <For each={logs()}>
                  {l => <div class={`text-neutral-600 ${l.cls}`}>{l.text}</div>}
                </For>
              </div>
              {/* quick reactions */}
              <div class="flex gap-1 mb-1 flex-wrap">
                {['nh', 'gg', 'wp', 'lol', 'wow', '...'].map(r =>
                  <button
                    class="text-10px px-2 py-0.5 rounded border border-white/10 text-neutral-600 hover:text-neutral-400 hover:border-neutral-600 active:text-zec-yellow active:border-zec-yellow transition-colors"
                    onClick={() => { send({ type: 'Chat', text: r }); log(`you: ${r}`, 'text-white') }}
                  >{r}</button>
                )}
              </div>
              <form class="flex gap-1 mb-2" onSubmit={(e) => {
                e.preventDefault()
                const input = e.currentTarget.querySelector('input') as HTMLInputElement
                const msg = input.value.trim()
                if (!msg) return
                input.value = ''
                // slash commands
                if (msg.startsWith('/nick ')) {
                  const newName = msg.slice(6).trim().slice(0, 20)
                  if (newName) {
                    setName(newName)
                    localStorage.setItem('poker_nickname', newName)
                    send({ type: 'Rename', name: newName }) // ← propagate to the opponent (was local-only)
                    log(`nickname set to ${newName}`, 'text-neutral-400')
                  }
                  return
                }
                send({ type: 'Chat', text: msg })
                log(`you: ${msg}`, 'text-white')
              }}>
                <input
                  class="input-field flex-1 text-10px py-0.5 px-2"
                  placeholder="chat..."
                  maxLength={200}
                />
                <button type="submit" class="text-10px px-2 py-0.5 rounded border border-white/15 text-neutral-600 hover:text-neutral-400">send</button>
              </form>
              </div>{/* end sidebar */}
            </div>
          </Show>

          {/* settlement view — on-chain payout in progress / complete / failed */}
          <Show when={view() === 'settlement'}>
            <div class="m-auto w-full max-w-md p-4">
              <div class="text-center mb-3">
                <div class="text-zec-yellow text-12px uppercase tracking-2px">table closed</div>
                <div class="text-neutral-500 text-11px mt-1">settling on-chain</div>
                <Show when={settleReason()}>
                  <div class="text-neutral-400 text-11px mt-1">{settleReason()}</div>
                </Show>
              </div>

              <div class="rounded-md border border-white/10 bg-zec-surface p-3 mb-3">
                <div class="text-neutral-500 text-11px uppercase tracking-wider mb-2">payout plan</div>
                <For each={settlePlan()}>
                  {(line) => {
                    const isMe = line.seat === mySeat()
                    const zec = (line.amount_zat / 1e8).toFixed(8).replace(/0+$/, '').replace(/\.$/, '')
                    return (
                      <div class="flex items-center justify-between text-10px tabular py-1">
                        <span class={isMe ? 'c-green' : 'text-neutral-400'}>
                          seat {line.seat}{isMe ? ' (you)' : ''} → {line.address.slice(0, 12)}…{line.address.slice(-6)}
                        </span>
                        <span class={isMe ? 'c-zec-yellow font-500' : 'text-neutral-500'}>{zec} ZEC</span>
                      </div>
                    )
                  }}
                </For>
              </div>

              <Show when={settleStatus().phase === 'preparing'}>
                <div class="text-center py-4">
                  <div class="i-lucide-loader-2 animate-spin h-6 w-6 mx-auto text-zec-yellow" />
                  <div class="text-neutral-400 text-10px mt-2">preparing on-chain payout…</div>
                  <div class="text-neutral-600 text-11px mt-1">building the multisig transaction (~3-5s)</div>
                </div>
              </Show>

              <Show when={settleStatus().phase === 'pending' || settleStatus().phase === 'signing'}>
                <div class="text-center text-11px tabular text-neutral-500 mb-2">
                  {mySeat() === settlePrioritySeat()
                    ? <>you have <span class="c-zec-yellow">{settleFallbackTick()}s</span> to approve before the signer flips to your opponent</>
                    : <>opponent can sign for <span class="c-zec-yellow">{settleFallbackTick()}s</span>; after that you can take over</>
                  }
                </div>
                <Show when={mySeat() === settlePrioritySeat()}>
                  <button
                    class="w-full btn btn-primary text-11px py-2"
                    disabled={settleStatus().phase === 'signing'}
                    onClick={() => {
                      setSettleStatus({ phase: 'signing' })
                      const code = roomCode()
                      const relayUrl = settleFrostRelay() || 'wss://zrelay.rotko.net'
                      void requestPokerSign({
                        relayUrl,
                        roomCode: settleRelayRoom(),
                        plan: settlePlan().map(p => ({ address: p.address, amount_zat: p.amount_zat })),
                        feeZat: 10_000,
                        multisigLabel: `POKER-${code}`,
                      }).then(res => {
                        if (!res.success) {
                          log(`zafu sign failed: ${res.error}`, 'c-red')
                          // back to pending so user can retry; PayoutFailed will land via WS too
                          setSettleStatus({ phase: 'pending' })
                        } else {
                          log('signing shares sent — waiting for escrow to broadcast', 'c-green')
                        }
                      })
                    }}
                  >
                    {settleStatus().phase === 'signing' ? 'signing…' : 'approve payout in zafu'}
                  </button>
                </Show>
                <Show when={mySeat() !== settlePrioritySeat()}>
                  <div class="text-center py-3">
                    <div class="i-lucide-loader-2 animate-spin mx-auto h-5 w-5 text-zec-yellow" />
                    <div class="text-neutral-400 text-10px mt-2">waiting for opponent to approve payout</div>
                  </div>
                </Show>
              </Show>

              <Show when={settleStatus().phase === 'complete'}>
                {(() => {
                  const status = settleStatus()
                  if (status.phase !== 'complete') return null
                  return (
                    <div class="text-center">
                      <div class="text-green-400 text-11px mb-2">✓ paid out on-chain</div>
                      <div class="text-neutral-500 text-11px uppercase mb-1">tx</div>
                      <div class="font-mono text-11px text-zec-yellow break-all px-2 mb-3">{status.txid}</div>
                      <button
                        class="w-full btn btn-secondary text-10px py-1.5"
                        onClick={() => {
                          setView('casino')
                          setSettleStatus({ phase: 'pending' })
                          setSettlePlan([])
                          setSettleRelayRoom('')
                          setSettlePrioritySeat(-1)
                          setSettleReason('')
                          history.replaceState(null, '', '/')
                        }}
                      >return to lobby</button>
                    </div>
                  )
                })()}
              </Show>

              <Show when={settleStatus().phase === 'failed'}>
                {(() => {
                  const status = settleStatus()
                  if (status.phase !== 'failed') return null
                  return (
                    <div class="text-center">
                      <div class="text-red-400 text-11px mb-2">✗ payout failed</div>
                      <div class="text-neutral-400 text-11px mb-3 break-words">{status.reason}</div>
                      <Show when={mySeat() === settlePrioritySeat()}>
                        <button
                          class="w-full btn btn-primary text-11px py-2"
                          onClick={() => setSettleStatus({ phase: 'pending' })}
                        >retry approval</button>
                      </Show>
                    </div>
                  )
                })()}
              </Show>
            </div>
          </Show>

          </div>{/* end main content */}

          <div class="shrink-0 text-center py-1.5 text-10px text-neutral-600 uppercase tracking-widest">zk.poker</div>
    </div>
  )
}

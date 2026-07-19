/**
 * WebSocket adapter — bridges relay transport + P2P game engine (poker-pvm WASM)
 * to the existing App.tsx which expects createSocket API.
 *
 * identity: zafu wallet (ed25519 delegation) or anonymous (ephemeral ed25519).
 * encryption: ephemeral x25519 DH → AES-256-GCM. all messages encrypted.
 * signing: session key signs every action (non-repudiable log for disputes).
 */

import { createSignal } from 'solid-js'
import type { ServerMsg } from './types'
import { createRelayTransport, getRoomFromUrl, setRoomInUrl } from './transport'
import { relayBase } from './config'
import { createGame, loadWasmEngine } from './game'
import { zid } from './zid'
import type { ZidIdentity } from './zid'
import { signKeyExchange } from './identity'
// SessionIdentity is now ZidIdentity
type SessionIdentity = ZidIdentity & { sessionPubKey: string; nick: string }
import type { WireMessage } from './transport'
import { createMedia } from './media'
import type { MediaState } from './media'

export type SendFn = (data: Record<string, unknown>) => void

export function createSocket(onMsg: (msg: ServerMsg) => void) {
  const [connected, setConnected] = createSignal(false)
  const [encSignal, setEncSignal] = createSignal(false)
  const [identity, setIdentity] = createSignal<SessionIdentity | null>(null)
  let game: ReturnType<typeof createGame> | null = null
  let transport: ReturnType<typeof createRelayTransport> | null = null
  let announced = false
  let media: MediaState | null = null
  // one wallet delegation per page session — cached so a re-join / double sit-down
  // reuses it instead of popping the wallet a second time.
  let cachedZid: ZidIdentity | null = null
  // rules-handshake self-heal: the host proposes and the guest auto-accepts, but a single
  // dropped relay frame would otherwise deadlock both sides forever ("waiting for opponent
  // to confirm"). We keep re-sending (idempotent — the negotiate layer guards on gameStarted)
  // until the accept lands, then stop.
  let rulesAgreedLocal = false
  let hostRulesRetry: ReturnType<typeof setInterval> | null = null
  let hostRetryStarted = false
  function stopHostRulesRetry() { if (hostRulesRetry) { clearInterval(hostRulesRetry); hostRulesRetry = null } }
  // staked (real-money) table: escrow control frames flow over the /p2p `srv`
  // channel to the server coordinator. false for free-play (no escrow at all).
  let isStaked = false

  /** direct WebSocket to server (centralized mode, not P2P) */
  async function connectDirect(name: string, pubkey?: string, zcashAddress?: string) {
    const room = getRoomFromUrl()
    if (!room) return

    let ws: WebSocket
    let intentionalClose = false
    let reconnectAttempts = 0
    const maxRetries = 10

    function open() {
      // relayBase(): user-selectable relay origin (default same-origin). This `/{room}/ws`
      // path is a legacy alias kept for local/dev; prod dials `/p2p` via transport.ts.
      ws = new WebSocket(`${relayBase()}/${room}/ws`)

      ws.onopen = () => {
        setConnected(true)
        reconnectAttempts = 0
        // pubkey gates reconnect server-side: a later reload with a different pubkey is
        // refused, defeating name-based seat hijack. Falls back to name-only if undefined.
        // re-sent verbatim on every (re)connect — the server matches name+pubkey to the
        // disconnected seat and replays game/deposit/settlement state.
        const join: Record<string, unknown> = { type: 'Join', name }
        if (pubkey) join.pubkey = pubkey
        if (zcashAddress) join.zcash_address = zcashAddress
        ws.send(JSON.stringify(join))
      }

      ws.onmessage = (ev) => {
        try {
          const msg = JSON.parse(ev.data)
          // terminal states: the room is being torn down server-side, so stop reconnecting
          // (a reconnect after cleanup would spawn a fresh empty room).
          if (msg.type === 'PayoutComplete' || msg.type === 'PayoutFailed') intentionalClose = true
          onMsg(msg)
        } catch {}
      }

      ws.onclose = () => {
        setConnected(false)
        if (intentionalClose) return
        if (reconnectAttempts < maxRetries) {
          reconnectAttempts++
          const delay = Math.min(reconnectAttempts, 5) * 1000
          console.log(`[ws] disconnected, reconnecting in ${delay}ms (attempt ${reconnectAttempts}/${maxRetries})`)
          setTimeout(open, delay)
        } else {
          console.log('[ws] reconnect attempts exhausted')
          onMsg({ type: 'Error', message: 'connection lost — reload to rejoin' })
        }
      }
    }

    open()

    return {
      send: (data: Record<string, unknown>) => {
        if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(data))
      },
      close: () => { intentionalClose = true; ws.close() },
    }
  }

  let directWs: { send: (data: Record<string, unknown>) => void; close: () => void } | null = null

  async function connect(name: string, customRules?: { smallBlind: number; bigBlind: number; buyin: number }, staked?: boolean) {
    isStaked = !!staked
    // Idempotent (re)connect: if a previous transport is still around (double "sit down",
    // a remount), disconnect it FIRST so its relay socket frees its seat instead of lingering
    // as a phantom opponent / triggering "room full". Reset per-session state for the new table.
    if (transport) { try { transport.disconnect() } catch {} transport = null }
    game = null
    announced = false
    stopHostRulesRetry()
    rulesAgreedLocal = false
    hostRetryStarted = false
    // P2P over the blind relay: both clients run the engine + mental-poker
    // ceremony locally and exchange only ciphertext through the server relay.
    // the operator never sees cards, and the two players never open a socket to
    // each other (no peer IP leak). the server engine at /{code}/ws is unused.
    onMsg({ type: 'Status', phase: 'connecting', message: 'loading game engine...' })
    const wasmOk = await loadWasmEngine()
    if (!wasmOk) onMsg({ type: 'Error', message: 'game engine failed to load — using fallback' })

    const room = getRoomFromUrl()

    // session identity via zid SDK (zafu wallet, or ephemeral ed25519)
    if (!cachedZid) cachedZid = await zid.connect({ appName: 'zkbtc.org', tradingMode: true })
    const zidIdentity = cachedZid
    // Refresh the cached identity's name too, or a nick change never takes effect
    // (cachedZid pins the name from the first connect for the double-sign fix).
    if (name) { zid.setName(name); zidIdentity.name = name }
    const sess: SessionIdentity = {
      ...zidIdentity,
      sessionPubKey: zidIdentity.pubkey,
      nick: zidIdentity.name,
    }
    setIdentity(sess)
    if (zidIdentity.mode === 'zafu') name = zidIdentity.name

    // host/guest is decided by the relay-assigned seat (join order), NOT the URL:
    // whoever joins the relay room first is seat 0 (host). the game is built once,
    // as soon as we learn our seat. (falls back to URL if the relay omits a seat.)
    let mySeat = -1

    function buildGame(isHost: boolean) {
      if (game) return
      // fresh handshake state for this table (guards against stale flags on rejoin)
      rulesAgreedLocal = false
      hostRetryStarted = false
      stopHostRulesRetry()
      // media: WebRTC voice/video (opt-in, direct P2P, signaling over the relay)
      media = createMedia((msg) => transport!.send(msg), !isHost)
      game = createGame(transport!, isHost, name, {
        onMsg,
        onLog: (t) => console.log('[game]', t),
        onRulesProposed: (rules, fromSelf) => {
          // Don't re-flash the proposal prompt once we've already agreed (host re-proposals
          // during self-heal would otherwise make the guest's UI flicker).
          if (!(rulesAgreedLocal && !fromSelf)) {
            onMsg({ type: 'RulesProposed', buyin: rules.buyin, smallBlind: rules.smallBlind, bigBlind: rules.bigBlind, fromSelf })
          }
          // Guest auto-accepts the host's rules (they chose to join this table). Re-accept on
          // EVERY proposal — if a prior accept was dropped, the host re-proposes and this heals it.
          if (!fromSelf && !isHost) setTimeout(() => game?.acceptRules(), 400)
          // Host: after proposing, keep re-proposing until the guest's accept arrives, so a
          // single dropped frame can't strand the table on "waiting for opponent to confirm".
          if (fromSelf && isHost && !hostRetryStarted) {
            hostRetryStarted = true
            let tries = 0
            hostRulesRetry = setInterval(() => {
              if (rulesAgreedLocal || tries++ >= 8) { stopHostRulesRetry(); return }
              game?.proposeRules(rules)
            }, 2500)
          }
        },
        onRulesAccepted: () => { rulesAgreedLocal = true; stopHostRulesRetry(); onMsg({ type: 'RulesAccepted' }) },
        onEscrowReady: (addr) => onMsg({ type: 'RoomInfo', code: room ?? '', jury_nodes: 5, jury_threshold: 3, escrow: addr }),
        onDepositConfirmed: () => {},
        onTimerTick: (s) => { if (s >= 0) onMsg({ type: 'TimerTick', secondsLeft: s }) },
      }, sess, customRules, !!staked)
    }

    transport = createRelayTransport(
      (msg: WireMessage) => {
        // media signaling filter
        if (msg.t === '_sdp' || msg.t === '_ice') { media?.handleSignal(msg); return }
        // chat filter
        if (msg.t === 'chat') {
          const text = (msg.d as any)?.text
          if (text) onMsg({ type: 'Chat', from: 'opp', text })
          return
        }
        game?.onPeerMessage(msg)
      },
      (event, data, seat) => {
        switch (event) {
          case 'joined':
            setConnected(true)
            if (mySeat < 0) {
              const relaySeat = typeof seat === 'number' ? seat : (room ? 1 : 0)
              // Seat/role STABILITY across reconnect. The relay assigns seats by live join
              // order and collapses the slot list on disconnect, so a reloading HOST would
              // rejoin as seat 1 and flip host<->guest (breaking the shuffle A/B roles →
              // nobody deals). The relay forwards blind between the two peers regardless of
              // seat, so the seat only decides OUR isHost / shuffle role — pin the first seat
              // we got for this room (keyed by nick so two tabs in one browser don't clash)
              // and reuse it on reload so host stays host and guest stays guest.
              const rc = (data ?? room) as string | undefined
              const seatKey = rc ? `poker_seat:${rc}:${name}` : null
              const pinned = seatKey ? localStorage.getItem(seatKey) : null
              mySeat = pinned !== null ? parseInt(pinned, 10) : relaySeat
              if (seatKey && pinned === null) localStorage.setItem(seatKey, String(mySeat))
              console.log('[ws] joined room:', data, 'seat:', mySeat, pinned !== null ? '(pinned)' : `(relay ${relaySeat})`)
              buildGame(mySeat === 0)
              onMsg({ type: 'Seated', seat: mySeat, name })
              // guest (joined second) announces immediately; host announces when
              // the opponent arrives (opponent_joined below).
              if (mySeat === 1) game?.announce()
            }
            if (data) {
              setRoomInUrl(data)
              onMsg({ type: 'RoomInfo', code: data, jury_nodes: 5, jury_threshold: 3, escrow: '' })
              onMsg({ type: 'InviteLink', url: '/' + data })
            }
            break
          case 'opponent_joined':
            console.log('[ws] opponent_joined, mySeat:', mySeat, 'announced:', announced)
            onMsg({ type: 'OpponentJoined', seat: mySeat === 0 ? 1 : 0, name: 'opponent' })
            if (mySeat === 0 && !announced) {
              announced = true
              console.log('[ws] host announcing (once)')
              game?.announce()
            }
            break
          case 'opponent_left':
            onMsg({ type: 'OpponentLeft', seat: mySeat === 0 ? 1 : 0 })
            break
          case 'opponent_disconnected':
            onMsg({ type: 'OpponentDisconnected', seat: mySeat === 0 ? 1 : 0, reconnect_secs: parseInt(data ?? '60') })
            game?.pauseTimer()
            break
          case 'opponent_reconnected':
            onMsg({ type: 'OpponentReconnected', seat: mySeat === 0 ? 1 : 0 })
            game?.resumeTimer()
            break
          case 'encrypted':
            setEncSignal(true)
            onMsg({ type: 'Status', phase: 'encrypting', message: 'encrypted channel established' })
            break
          case 'error':
            onMsg({ type: 'Error', message: data ?? 'unknown error' })
            break
        }
      },
      sess, // pass identity for authenticated key exchange
      undefined, // default reconnect config
      (sm) => {
        // inbound server escrow control frame (staked tables): RoomInfo (with
        // frost coords → triggers DKG), DepositStatus, PayoutSigningRequest,
        // PayoutComplete/Failed. Dispatch straight into the App message handler.
        const m = sm as ServerMsg & { type?: string }
        // Only a REAL-MONEY RoomInfo flips this connection to staked. Prefer the server's
        // explicit `staked` flag; fall back to the presence of FROST DKG coords (older relay).
        // A bare RoomInfo (free-play / relay bookkeeping) must NOT enable escrow control frames.
        if (m?.type === 'RoomInfo') {
          const ri = m as ServerMsg & { staked?: boolean; frost_relay_url?: string; frost_room_code?: string }
          if (ri.staked === true || (ri.frost_relay_url && ri.frost_room_code)) isStaked = true
        }
        // feed per-seat payout addresses (pinned on-chain, surfaced by the server) to the
        // game so both peers can build the identical co-signed settlement message at game over.
        if (m?.type === 'DepositStatus' && Array.isArray((m as any).seat_payout_addresses)) {
          game?.setPayoutAddresses((m as any).seat_payout_addresses)
        }
        onMsg(m as ServerMsg)
      },
    )

    // Transport-level reliability signals (added in transport.ts):
    // • onDesync: the seq/ack layer detected loss/reorder it could not repair
    //   (e.g. the peer acked a seq we never sent). Surface it — game.ts's own
    //   per-hand action-seq guard voids/resyncs the hand; this catches the case
    //   where a frame vanished BELOW the game layer so no engine seq gap appears.
    // • onFatal: crypto handshake did not complete symmetrically (or X25519 init
    //   failed) → refuse to run a half-plaintext staked table.
    transport.onDesync((reason) => {
      console.warn('[ws] transport desync:', reason)
      onMsg({ type: 'Status', phase: 'desync',
        message: 'connection desync detected — the current hand may be void; your deposit is safe.' })
    })
    transport.onFatal((reason) => {
      console.error('[ws] transport fatal:', reason)
      onMsg({ type: 'Error', message: `secure channel failed: ${reason}` })
    })

    transport.connect(room, name)
  }

  const send: SendFn = (data) => {
    // direct server mode: send JSON to server WebSocket
    if (directWs) {
      directWs.send(data)
      return
    }
    // P2P relay mode: route through game engine
    if (data['type'] === 'Action') {
      game?.act((data['action'] as string).toLowerCase(), (data['amount'] as number) || 0)
    } else if (data['type'] === 'StartHand') {
      game?.dealHand()
    } else if (data['type'] === 'ProposeRules') {
      game?.proposeRules(data as any)
    } else if (data['type'] === 'AcceptRules') {
      game?.acceptRules()
    } else if (data['type'] === 'DkgComplete') {
      // escrow control frame → SERVER coordinator over the /p2p `srv` channel
      // (NOT the peer). the server records this seat's agreed escrow UA and,
      // once both seats match, pins escrow_address + re-broadcasts RoomInfo.
      transport?.sendServer(data)
    } else if (data['type'] === 'EscrowFault') {
      // client-detected escrow problem → server coordinator over /p2p `srv` channel,
      // which forwards it to the escrow's durable journal (client_fault event).
      // only meaningful on staked tables; harmless no-op otherwise.
      if (isStaked) transport?.sendServer({ type: 'EscrowFault', phase: data['phase'], detail: data['detail'] })
    } else if (data['type'] === 'Chat') {
      transport?.send({ t: 'chat', d: { text: data['text'] } })
    } else if (data['type'] === 'Rename') {
      // propagate an in-game nick change to the peer. A dedicated `rename` frame (NOT a re-`seated`,
      // which would trigger the reconnect resync) — the peer just updates the displayed opponent name.
      transport?.send({ t: 'rename', d: { name: data['name'] } })
    } else if (data['type'] === 'Leave') {
      // staked table: tell the server coordinator to settle (co-signed game-over
      // → deposit refund / payout) over the `srv` channel, then stay connected to
      // receive PayoutSigningRequest/PayoutComplete. free-play: just drop the
      // relay socket so the opponent is notified.
      if (isStaked) {
        transport?.sendServer({ type: 'Leave' })
      } else {
        transport?.disconnect()
      }
    } else {
      // no route matched — surface it instead of silently dropping the frame
      console.warn('[ws] unrouted send, frame dropped:', data['type'], data)
    }
  }

  return { connected, connect, send, identity, encrypted: encSignal, media: () => media }
}

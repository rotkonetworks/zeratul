import { createSignal, For, Show, createEffect, onCleanup } from 'solid-js'
import { createSocket } from './ws'
import { Card } from './Card'
import type { ServerMsg, CardJson, ValidAction } from './types'

export default function App() {
  const [view, setView] = createSignal<'lobby' | 'waiting' | 'game'>('lobby')
  const [name, setName] = createSignal('')
  const [mySeat, setMySeat] = createSignal(-1)
  const [oppName, setOppName] = createSignal('\u2014')
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

  const opp = () => mySeat() === 0 ? 1 : 0
  const myStack = () => stacks()[mySeat()] ?? 0
  const oppStack = () => stacks()[opp()] ?? 0
  const myBet = () => bets()[mySeat()] ?? 0
  const oppBet = () => bets()[opp()] ?? 0
  const isMyTurn = () => acting() === mySeat()

  function log(text: string, cls = '') {
    setLogs(l => [...l.slice(-60), { text, cls }])
  }

  function onMsg(msg: ServerMsg) {
    switch (msg.type) {
      case 'Seated':
        setMySeat(msg.seat)
        setView('waiting')
        break
      case 'Waiting':
        setView('waiting')
        break
      case 'OpponentJoined':
        setOppName(msg.name)
        break
      case 'OpponentLeft':
        setOppName('\u2014')
        setActions([])
        setView('waiting')
        log('opponent left')
        break
      case 'HandStarted':
        setView('game')
        setStacks(msg.stacks)
        setBets([0, 0])
        setButton(msg.button)
        setHandNum(msg.hand_number)
        setBoard([])
        setPot(0)
        setActions([])
        setOppRevealed(false)
        setOppCards(null)
        if (msg.your_cards) {
          setMyCards(msg.your_cards)
        }
        log(`hand #${msg.hand_number}`, 'c-green')
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
        setActing(msg.seat)
        if (msg.seat === mySeat()) {
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
        const s = [...stacks()]
        s[msg.seat] = msg.new_stack
        setStacks(s)
        const b = [...bets()]
        if (msg.action === 'bet' || msg.action === 'raise') b[msg.seat] = msg.amount
        else if (msg.action === 'call') b[msg.seat] = Math.max(...b)
        setBets(b)
        const who = msg.seat === mySeat() ? 'you' : 'opp'
        const amt = msg.amount > 0 && (msg.action === 'bet' || msg.action === 'raise') ? ` ${msg.amount}` : ''
        log(`${who}: ${msg.action}${amt}`)
        break
      }
      case 'CommunityCards':
        setBoard(msg.cards)
        setBets([0, 0])
        log(`${msg.phase}: ${msg.cards.map(c => c.rank + c.suit).join(' ')}`, 'c-green')
        break
      case 'PotUpdate':
        setPot(msg.pots.reduce((s, p) => s + p.amount, 0))
        break
      case 'Showdown':
        for (const [seat, cards] of msg.hands) {
          if (seat === opp()) { setOppCards(cards); setOppRevealed(true) }
        }
        log('showdown', 'c-green')
        break
      case 'PotAwarded':
        log(`${msg.seat === mySeat() ? 'you' : 'opp'} wins ${msg.amount}`, 'c-zec-yellow font-500')
        break
      case 'HandComplete':
        setStacks(msg.stacks)
        setBets([0, 0])
        setActions([])
        setActing(-1)
        break
      case 'Error':
        log(`err: ${msg.message}`)
        break
    }
  }

  const { connected, connect, send } = createSocket(onMsg)

  function sit() {
    const n = name().trim() || 'anon'
    connect(n)
  }

  function act(action: string, amount?: number) {
    send({ type: 'Action', action, ...(amount !== undefined && { amount }) })
    setActions([])
  }

  // auto-scroll log
  let logEl!: HTMLDivElement
  createEffect(() => {
    logs()
    if (logEl) logEl.scrollTop = logEl.scrollHeight
  })

  return (
    <div class="min-h-screen flex items-center justify-center p-4 bg-zec-dark font-sans text-white">
      <div class="w-full max-w-160">
        <div class="panel">
          {/* titlebar */}
          <div class="titlebar">
            <span class="text-zec-yellow text-14px">{'\u2666'}</span>
            <span class="flex-1 text-center text-zec-yellow">zk.poker</span>
            <span class={`w-2 h-2 rounded-full ${connected() ? 'bg-green-500' : 'bg-neutral-600'}`} />
          </div>

          {/* lobby */}
          <Show when={view() === 'lobby'}>
            <div class="p-8 text-center">
              <div class="text-zec-yellow text-10px font-semibold uppercase tracking-3px mb-5">
                heads-up no-limit hold'em
              </div>
              <div class="text-neutral-500 text-11px tracking-wider mb-6">
                5 / 10 blinds &middot; 1,000 buy-in
              </div>
              <div class="flex items-center justify-center gap-2">
                <input
                  class="input-field w-48 text-center"
                  placeholder="name"
                  maxLength={16}
                  spellcheck={false}
                  value={name()}
                  onInput={e => setName(e.currentTarget.value)}
                  onKeyDown={e => e.key === 'Enter' && sit()}
                  autofocus
                />
                <button class="btn btn-primary" onClick={sit}>sit down</button>
              </div>
            </div>
          </Show>

          {/* waiting */}
          <Show when={view() === 'waiting'}>
            <div class="p-10 text-center">
              <div class="text-zec-yellow text-11px uppercase tracking-2px mb-4">
                waiting for opponent
              </div>
              <div class="flex items-end justify-center gap-1 h-6">
                <For each={[0,.07,.14,.21,.28,.35]}>
                  {d => <div class="w-1 rounded-sm bg-zec-yellow animate-pulse" style={`animation-delay:${d}s; height: 60%`} />}
                </For>
              </div>
            </div>
          </Show>

          {/* game */}
          <Show when={view() === 'game'}>
            <div class="px-2">
              {/* status bar */}
              <div class="flex justify-between px-2 py-1.5 text-9px text-neutral-500 uppercase tracking-wider">
                <span>hand #{handNum()}</span>
                <span>{button() === mySeat() ? 'you deal' : 'opp deals'}</span>
              </div>

              {/* felt */}
              <div class="bg-zec-felt border-2 border-zec-feltb rounded-25 px-5 py-6 relative" style="min-height: 260px; box-shadow: inset 0 2px 20px rgba(0,0,0,0.4)">

                {/* opponent (top) */}
                <div class="absolute top--4 left-50% -translate-x-50% text-center w-44">
                  <div class={`inline-block px-3 py-1 bg-zec-surface border ${acting() === opp() ? 'border-zec-yellow shadow-[0_0_8px_rgba(244,183,40,0.3)]' : 'border-neutral-800'}`}>
                    <div class={`text-9px font-semibold uppercase tracking-wider ${acting() === opp() ? 'text-zec-yellow' : 'text-neutral-500'}`}>
                      {oppName()}
                    </div>
                    <div class="font-mono text-13px text-zec-yellow">{oppStack()}</div>
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
                  <div class="font-mono text-11px text-neutral-400 mt-0.5 h-4">{oppBet() > 0 ? oppBet() : ''}</div>
                </div>

                {/* dealer chip */}
                <Show when={button() === mySeat()}>
                  <div class="absolute bottom-12 rounded-full w-5.5 h-5.5 bg-zec-yellow text-black text-9px font-bold leading-5.5 text-center border-2 border-zec-gold z-5"
                    style="left: calc(50% + 55px)">D</div>
                </Show>
                <Show when={button() === opp()}>
                  <div class="absolute top-12 rounded-full w-5.5 h-5.5 bg-zec-yellow text-black text-9px font-bold leading-5.5 text-center border-2 border-zec-gold z-5"
                    style="left: calc(50% + 55px)">D</div>
                </Show>

                {/* board */}
                <div class="flex gap-1.5 justify-center my-13">
                  <For each={board()}>
                    {c => <Card card={c} size="lg" />}
                  </For>
                </div>

                {/* pot */}
                <div class="text-center font-mono text-14px font-500 text-zec-yellow min-h-5">
                  {pot() > 0 ? pot() : ''}
                </div>

                {/* you (bottom) */}
                <div class="absolute bottom--4 left-50% -translate-x-50% text-center w-44">
                  <div class="font-mono text-11px text-neutral-400 mb-0.5 h-4">{myBet() > 0 ? myBet() : ''}</div>
                  <div class="flex gap-1 justify-center mb-1.5">
                    <Show when={myCards()}>
                      <Card card={myCards()![0]} />
                      <Card card={myCards()![1]} />
                    </Show>
                  </div>
                  <div class={`inline-block px-3 py-1 bg-zec-surface border ${acting() === mySeat() ? 'border-zec-yellow shadow-[0_0_8px_rgba(244,183,40,0.3)]' : 'border-neutral-800'}`}>
                    <div class="font-mono text-13px text-zec-yellow">{myStack()}</div>
                    <div class={`text-9px font-semibold uppercase tracking-wider ${acting() === mySeat() ? 'text-zec-yellow' : 'text-neutral-500'}`}>
                      {name() || 'you'}
                    </div>
                  </div>
                </div>
              </div>

              {/* actions */}
              <div class="flex gap-1.5 justify-center items-center py-3 min-h-12 flex-wrap">
                <Show when={isMyTurn() && actions().length > 0} fallback={
                  <Show when={acting() >= 0 && !isMyTurn()}>
                    <span class="text-neutral-600 text-10px uppercase tracking-wider">opponent to act</span>
                  </Show>
                }>
                  <For each={actions()}>
                    {a => {
                      if (a.kind === 'fold')
                        return <button class="btn btn-danger" onClick={() => act('fold')}>fold</button>
                      if (a.kind === 'check')
                        return <button class="btn" onClick={() => act('check')}>check</button>
                      if (a.kind === 'call')
                        return <button class="btn btn-primary" onClick={() => act('call')}>call {a.min_amount}</button>
                      if (a.kind === 'bet')
                        return <>
                          <input class="input-field w-20 text-center" type="number"
                            min={a.min_amount} max={a.max_amount}
                            value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                          <button class="btn" onClick={() => act('bet', raiseVal())}>bet</button>
                        </>
                      if (a.kind === 'raise')
                        return <>
                          <Show when={!actions().some(x => x.kind === 'bet')}>
                            <input class="input-field w-20 text-center" type="number"
                              min={a.min_amount} max={a.max_amount}
                              value={raiseVal()} onInput={e => setRaiseVal(+e.currentTarget.value)} />
                          </Show>
                          <button class="btn" onClick={() => act('raise', raiseVal())}>raise</button>
                        </>
                      if (a.kind === 'allin')
                        return <button class="btn btn-allin" onClick={() => act('allin')}>all in</button>
                      return null
                    }}
                  </For>
                </Show>
              </div>

              {/* log */}
              <div ref={logEl!} class="bg-zec-surface border border-neutral-800 p-2 max-h-28 overflow-y-auto font-mono text-10px mb-2 leading-relaxed">
                <For each={logs()}>
                  {l => <div class={`text-neutral-600 ${l.cls}`}>{l.text}</div>}
                </For>
              </div>
            </div>
          </Show>

          <div class="text-center py-1.5 text-8px text-neutral-600 uppercase tracking-widest">rotko networks</div>
        </div>
      </div>
    </div>
  )
}

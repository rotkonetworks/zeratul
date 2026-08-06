import { createSignal, For, Show, onMount, onCleanup } from 'solid-js'
import type { ZidIdentity, ContactRef } from './zid/types'
import { upsertContact, removeContact, getContactRefs } from './zid'

/**
 * Friends panel — Steam-style. Anchored on the ZID (pubkey), portable via the wallet.
 *
 * Opens on the `#/friends` hash route (self-contained overlay, like Tournaments). It reads the zid
 * identity from App via the `identity` prop so it can send play-requests (zafu mode) and pick wallet
 * contacts. The friend graph lives CLIENT-SIDE in zid contacts (localStorage) — never on the relay,
 * so the blind relay stays blind.
 *
 * PRIVACY: each wallet controls how much it exposes and to whom (presence + who-can-invite). Stored
 * per-device; presence enforcement lands with the P2P beacon (phase 2), but the prefs are the source
 * of truth from day one — nothing is shared beyond what these allow.
 */

type Visibility = 'friends' | 'nobody' | 'anyone'
interface Prefs {
  /** who may see you're online (once presence ships). default: friends. */
  presence: Visibility
  /** who may send you a play-request. default: friends. */
  invitesFrom: Visibility
}
const PREF_KEY = 'poker_privacy_v1'
const DEFAULT_PREFS: Prefs = { presence: 'friends', invitesFrom: 'friends' }
function loadPrefs(): Prefs {
  try {
    return { ...DEFAULT_PREFS, ...(JSON.parse(localStorage.getItem(PREF_KEY) || '{}') as Partial<Prefs>) }
  } catch {
    return { ...DEFAULT_PREFS }
  }
}
function savePrefs(p: Prefs) {
  try { localStorage.setItem(PREF_KEY, JSON.stringify(p)) } catch { /* ignore */ }
}

export default function Friends(props: { identity: ZidIdentity | null }) {
  const [open, setOpen] = createSignal(location.hash.replace(/^#/, '').startsWith('/friends'))
  const onHash = () => setOpen(location.hash.replace(/^#/, '').startsWith('/friends'))
  window.addEventListener('hashchange', onHash)
  onCleanup(() => window.removeEventListener('hashchange', onHash))
  const close = () => { window.location.hash = '' }

  const [contacts, setContacts] = createSignal<ContactRef[]>([])
  const refresh = async () => {
    try { setContacts(await getContactRefs(location.origin)) } catch { setContacts([]) }
  }
  onMount(refresh)

  // add by ZID (pubkey hex) + optional nickname
  const [zidInput, setZidInput] = createSignal('')
  const [nick, setNick] = createSignal('')
  const [err, setErr] = createSignal('')
  const add = () => {
    const pk = zidInput().trim().toLowerCase()
    if (!/^[0-9a-f]{64}$/.test(pk)) { setErr('paste a 64-char ZID (hex pubkey)'); return }
    upsertContact(pk, nick().trim() || pk.slice(0, 8))
    setZidInput(''); setNick(''); setErr('')
    void refresh()
  }

  // import contacts from the zafu wallet (one-way today; wallet-write is an extension follow-up)
  const [importing, setImporting] = createSignal(false)
  const importFromWallet = async () => {
    if (!props.identity?.pickContacts) { setErr('connect your zafu wallet to import contacts'); return }
    setImporting(true); setErr('')
    try {
      const picked = await props.identity.pickContacts({ purpose: 'Import friends into zk.poker', max: 50 })
      // picked refs are already known to zid for this app; surface them alongside stored contacts
      if (picked?.length) {
        const have = new Set(contacts().map(c => c.handle))
        setContacts([...contacts(), ...picked.filter(p => !have.has(p.handle))])
      }
    } catch { /* cancelled */ } finally { setImporting(false) }
  }

  const [sent, setSent] = createSignal<string | null>(null)
  const invite = async (c: ContactRef) => {
    if (!props.identity?.invite) { setErr('play-requests need your zafu wallet connected'); return }
    try {
      await props.identity.invite(c.handle, { type: 'poker-play-request', data: {}, ttl: 300 })
      setSent(c.handle); setTimeout(() => setSent(null), 1500)
    } catch { setErr('could not send the play-request') }
  }

  const remove = (c: ContactRef) => { removeContact(c.handle); void refresh() }

  const [prefs, setPrefs] = createSignal<Prefs>(loadPrefs())
  const setPref = (k: keyof Prefs, v: Visibility) => {
    const p = { ...prefs(), [k]: v }
    setPrefs(p); savePrefs(p)
  }

  const VisPicker = (p: { value: Visibility; onChange: (v: Visibility) => void; extra?: Visibility[] }) => (
    <div class="flex gap-1">
      <For each={['friends', 'nobody', ...(p.extra ?? [])] as Visibility[]}>{opt => (
        <button
          class={`text-9px px-2 py-0.5 rounded-full border ${p.value === opt ? 'bg-zec-yellow/15 text-zec-yellow border-zec-yellow/40' : 'bg-white/5 text-white/45 border-white/10'}`}
          onClick={() => p.onChange(opt)}
        >{opt}</button>
      )}</For>
    </div>
  )

  return (
    <Show when={open()}>
      <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4" onClick={close}>
        <div class="w-full max-w-md bg-zec-dark border border-white/12 rounded-lg p-5 text-white max-h-[85vh] overflow-y-auto" onClick={e => e.stopPropagation()}>
          <div class="flex items-center justify-between mb-1">
            <h2 class="text-14px text-zec-yellow tracking-wide">friends</h2>
            <button class="text-neutral-500 hover:text-white text-16px leading-none" onClick={close}>×</button>
          </div>
          <p class="text-10px text-neutral-500 mb-4 leading-relaxed">
            Add a friend by their ZID (their public identity) or import from your wallet, then invite
            them to play. Your friends list lives in your wallet — never on our servers.
          </p>

          {/* add by ZID */}
          <div class="flex gap-2 mb-2">
            <input class="input-field flex-1 text-11px font-mono" placeholder="friend's ZID (64-char hex)"
              value={zidInput()} onInput={e => setZidInput(e.currentTarget.value)} />
            <input class="input-field w-24 text-11px" placeholder="nickname"
              value={nick()} onInput={e => setNick(e.currentTarget.value)} />
            <button class="btn btn-primary text-11px px-3" onClick={add}>add</button>
          </div>
          <button class="text-10px text-zec-yellow/80 hover:text-zec-yellow mb-3 disabled:opacity-40"
            disabled={importing()} onClick={importFromWallet}>
            {importing() ? 'importing…' : '↓ import friends from your wallet'}
          </button>
          <Show when={err()}><div class="text-11px text-red-400 mb-2">{err()}</div></Show>

          {/* friends list */}
          <div class="flex flex-col gap-1.5 mb-5">
            <Show when={contacts().length === 0}>
              <div class="text-11px text-neutral-500 text-center py-4">no friends yet — add one above</div>
            </Show>
            <For each={contacts()}>{c => (
              <div class="flex items-center gap-2 p-2 rounded bg-black/40 border border-white/10">
                <div class="w-6 h-6 rounded-full bg-zec-yellow/15 text-zec-yellow flex items-center justify-center text-10px shrink-0">
                  {(c.name || '?').slice(0, 1).toUpperCase()}
                </div>
                <span class="text-12px text-neutral-200 truncate flex-1">{c.name || c.handle.slice(0, 10)}</span>
                <button class="text-10px px-2.5 py-1 rounded bg-green-500/15 text-green-400 border border-green-500/30 hover:bg-green-500/25 disabled:opacity-50"
                  onClick={() => invite(c)}>
                  {sent() === c.handle ? 'sent ✓' : 'invite'}
                </button>
                <button class="text-neutral-600 hover:text-red-400 text-13px shrink-0" title="remove friend"
                  onClick={() => remove(c)}>×</button>
              </div>
            )}</For>
          </div>

          {/* privacy — per-wallet, you control what you expose and to whom */}
          <div class="border-t border-white/10 pt-3">
            <div class="text-10px text-zec-text font-semibold uppercase tracking-wider mb-2">privacy</div>
            <div class="flex items-center justify-between mb-2">
              <span class="text-11px text-neutral-400">who sees you're online</span>
              <VisPicker value={prefs().presence} onChange={v => setPref('presence', v)} />
            </div>
            <div class="flex items-center justify-between">
              <span class="text-11px text-neutral-400">who can invite you</span>
              <VisPicker value={prefs().invitesFrom} onChange={v => setPref('invitesFrom', v)} extra={['anyone']} />
            </div>
            <p class="text-9px text-neutral-600 mt-2 leading-relaxed">
              These live only on this device. Presence (“online now”) is off until you enable it — and
              even then it’s shared peer-to-peer with the people you allow, never uploaded to a server.
            </p>
          </div>
        </div>
      </div>
    </Show>
  )
}

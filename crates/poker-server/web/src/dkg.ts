// poker escrow DKG via zafu's generic zafu_dkg_join API.
//
// All FROST/DKG/delete operations are gated by zafu on the 'frost' capability.
// Before any of these calls, we ensure the user has granted that capability
// to this origin via zafu_request_capability. The user sees a permission
// popup once (per browser profile, per origin) and approves; subsequent calls
// proceed without re-prompting.

/**
 * Ensure the user has granted the 'frost' capability to this origin. Idempotent:
 * if already granted, the zafu side resolves without a popup. Resolves to true
 * on grant, false on user denial / extension missing.
 *
 * Caches the result for the session to avoid repeated round-trips.
 */
let frostCapabilityGranted: boolean | null = null
export async function ensureFrostCapability(): Promise<boolean> {
  if (frostCapabilityGranted === true) { console.log('[poker-dkg] ensureFrostCapability: cached=true'); return true }
  const extId = findZafuExtensionId()
  console.log('[poker-dkg] ensureFrostCapability: extId=', extId)
  if (!extId) { console.warn('[poker-dkg] ensureFrostCapability: no zafu extension id'); return false }
  if (typeof chrome === 'undefined' || !chrome.runtime?.sendMessage) {
    console.warn('[poker-dkg] ensureFrostCapability: chrome.runtime.sendMessage unavailable'); return false
  }
  console.log('[poker-dkg] ensureFrostCapability: sending zafu_request_capability { capability: frost }')
  const granted = await new Promise<boolean>(resolve => {
    chrome.runtime.sendMessage(
      extId,
      { type: 'zafu_request_capability', capability: 'frost' },
      (resp: any) => {
        if (chrome.runtime.lastError) {
          console.error('[poker-dkg] ensureFrostCapability: lastError=', chrome.runtime.lastError.message)
          resolve(false)
          return
        }
        console.log('[poker-dkg] ensureFrostCapability: resp=', resp)
        resolve(!!resp?.granted || !!resp?.success)
      },
    )
  })
  console.log('[poker-dkg] ensureFrostCapability: granted=', granted)
  frostCapabilityGranted = granted
  return granted
}

export interface PokerDkgRequest {
  relayUrl: string
  roomCode: string
  threshold?: number
  maxSigners?: number
  /** wallet label prefix; zafu appends "-YYYY-MM-DD-HHMM" */
  labelPrefix?: string
  /** hide the resulting multisig from zafu's wallet UI (default true for poker) */
  hide?: boolean
}

export interface PokerDkgResult {
  success: true
  address: string
  orchardFvk: string
  roomCode: string
}

export interface PokerDkgError {
  success: false
  error: string
}

/** same discovery as identity.ts: window[Symbol.for('penumbra')] */
function findZafuExtensionId(): string | null {
  const providers = (window as any)[Symbol.for('penumbra')]
  if (!providers) { console.warn('[poker-dkg] findZafuExtensionId: window[Symbol.for("penumbra")] is missing — zafu not injected'); return null }
  const entries = Object.entries(providers)
  console.log('[poker-dkg] findZafuExtensionId: penumbra providers=', entries.map(e => e[0]))
  if (!entries.length) { console.warn('[poker-dkg] findZafuExtensionId: no providers'); return null }
  const [origin] = entries[0] as [string, unknown]
  const id = origin.replace('chrome-extension://', '').replace(/\/$/, '')
  console.log('[poker-dkg] findZafuExtensionId: id=', id)
  return id
}

export interface PokerSignRequest {
  relayUrl: string
  roomCode: string                              // FROST relay room (NOT the poker room code)
  plan: { address: string; amount_zat: number }[]
  feeZat?: number
  /** label prefix (e.g. "POKER-{code}") so zafu auto-picks the matching multisig */
  multisigLabel: string
}

export async function requestPokerSign(req: PokerSignRequest): Promise<{ success: boolean; signed?: boolean; error?: string }> {
  console.log('[poker-dkg] requestPokerSign: called req=', req)
  const extId = findZafuExtensionId()
  if (!extId) { console.error('[poker-dkg] requestPokerSign: zafu extension not detected'); return { success: false, error: 'zafu extension not detected' } }
  if (typeof chrome === 'undefined' || !chrome.runtime?.sendMessage) {
    console.error('[poker-dkg] requestPokerSign: chrome.runtime.sendMessage unavailable')
    return { success: false, error: 'chrome.runtime.sendMessage unavailable' }
  }
  if (!(await ensureFrostCapability())) {
    console.error('[poker-dkg] requestPokerSign: frost capability denied')
    return { success: false, error: 'frost capability denied' }
  }
  const msg = {
    type: 'zafu_frost_sign_orchard',
    relayUrl: req.relayUrl,
    roomCode: req.roomCode,
    plan: req.plan,
    feeZat: req.feeZat ?? 10_000,
    multisigLabel: req.multisigLabel,
  }
  console.log('[poker-dkg] requestPokerSign: sending zafu_frost_sign_orchard', msg)
  return new Promise(resolve => {
    chrome.runtime.sendMessage(
      extId,
      msg,
      (resp: any) => {
        if (chrome.runtime.lastError) {
          console.error('[poker-dkg] requestPokerSign: lastError=', chrome.runtime.lastError.message)
          resolve({ success: false, error: chrome.runtime.lastError.message ?? 'sendMessage failed' })
          return
        }
        console.log('[poker-dkg] requestPokerSign: zafu_frost_sign_orchard resp=', resp)
        resolve({ success: !!resp?.success, signed: !!resp?.signed, error: resp?.error })
      },
    )
  })
}

export interface PokerDeleteRequest {
  /** label prefix used at DKG (e.g. "POKER-{code}"); most-recent match is deleted */
  multisigLabel: string
  /** ms from now until deletion fires; 0 = immediate */
  delayMs?: number
}

/** schedule (or immediately do) deletion of a poker-table multisig vault on zafu.
 *  fired on PayoutComplete with a 24h delay so the user can verify on-chain first;
 *  the vault evaporates next time zafu's service worker wakes after the deadline. */
export async function requestDeletePokerMultisig(req: PokerDeleteRequest): Promise<{ success: boolean; error?: string }> {
  const extId = findZafuExtensionId()
  if (!extId) return { success: false, error: 'zafu extension not detected' }
  if (typeof chrome === 'undefined' || !chrome.runtime?.sendMessage) {
    return { success: false, error: 'chrome.runtime.sendMessage unavailable' }
  }
  if (!(await ensureFrostCapability())) {
    return { success: false, error: 'frost capability denied' }
  }
  return new Promise(resolve => {
    chrome.runtime.sendMessage(
      extId,
      {
        type: 'zafu_delete_multisig',
        multisigLabel: req.multisigLabel,
        delayMs: req.delayMs ?? 0,
      },
      (resp: any) => {
        if (chrome.runtime.lastError) {
          resolve({ success: false, error: chrome.runtime.lastError.message ?? 'sendMessage failed' })
          return
        }
        resolve({ success: !!resp?.success, error: resp?.error })
      },
    )
  })
}

export async function requestPokerDkg(req: PokerDkgRequest): Promise<PokerDkgResult | PokerDkgError> {
  console.log('[poker-dkg] requestPokerDkg: called req=', req)
  const extId = findZafuExtensionId()
  if (!extId) {
    console.error('[poker-dkg] requestPokerDkg: zafu extension not detected')
    return { success: false, error: 'zafu extension not detected' }
  }
  if (typeof chrome === 'undefined' || !chrome.runtime?.sendMessage) {
    console.error('[poker-dkg] requestPokerDkg: chrome.runtime.sendMessage unavailable')
    return { success: false, error: 'chrome.runtime.sendMessage unavailable' }
  }
  if (!(await ensureFrostCapability())) {
    console.error('[poker-dkg] requestPokerDkg: frost capability denied')
    return { success: false, error: 'frost capability denied' }
  }

  const msg = {
    type: 'zafu_dkg_join',
    relayUrl: req.relayUrl,
    roomCode: req.roomCode,
    threshold: req.threshold ?? 2,
    maxSigners: req.maxSigners ?? 3,
    labelPrefix: req.labelPrefix ?? 'POKER',
    hide: req.hide ?? true,
  }
  console.log('[poker-dkg] requestPokerDkg: sending zafu_dkg_join to', extId, msg)
  return new Promise(resolve => {
    chrome.runtime.sendMessage(
      extId,
      msg,
      (resp: any) => {
        if (chrome.runtime.lastError) {
          console.error('[poker-dkg] requestPokerDkg: lastError=', chrome.runtime.lastError.message)
          resolve({ success: false, error: chrome.runtime.lastError.message ?? 'sendMessage failed' })
          return
        }
        console.log('[poker-dkg] requestPokerDkg: zafu_dkg_join resp=', resp)
        if (resp?.success && resp.address && resp.orchardFvk) {
          resolve({
            success: true,
            address: resp.address,
            orchardFvk: resp.orchardFvk,
            roomCode: resp.roomCode ?? req.roomCode,
          })
          return
        }
        resolve({ success: false, error: resp?.error ?? 'zafu denied or returned no result' })
      },
    )
  })
}

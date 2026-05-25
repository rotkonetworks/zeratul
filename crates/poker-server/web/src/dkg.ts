// poker escrow DKG via zafu's generic zafu_dkg_join API.

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
  if (!providers) return null
  const entries = Object.entries(providers)
  if (!entries.length) return null
  const [origin] = entries[0] as [string, unknown]
  return origin.replace('chrome-extension://', '').replace(/\/$/, '')
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
  const extId = findZafuExtensionId()
  if (!extId) return { success: false, error: 'zafu extension not detected' }
  if (typeof chrome === 'undefined' || !chrome.runtime?.sendMessage) {
    return { success: false, error: 'chrome.runtime.sendMessage unavailable' }
  }
  return new Promise(resolve => {
    chrome.runtime.sendMessage(
      extId,
      {
        type: 'zafu_frost_sign_orchard',
        relayUrl: req.relayUrl,
        roomCode: req.roomCode,
        plan: req.plan,
        feeZat: req.feeZat ?? 10_000,
        multisigLabel: req.multisigLabel,
      },
      (resp: any) => {
        if (chrome.runtime.lastError) {
          resolve({ success: false, error: chrome.runtime.lastError.message ?? 'sendMessage failed' })
          return
        }
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
  const extId = findZafuExtensionId()
  if (!extId) {
    return { success: false, error: 'zafu extension not detected' }
  }
  if (typeof chrome === 'undefined' || !chrome.runtime?.sendMessage) {
    return { success: false, error: 'chrome.runtime.sendMessage unavailable' }
  }

  return new Promise(resolve => {
    chrome.runtime.sendMessage(
      extId,
      {
        type: 'zafu_dkg_join',
        relayUrl: req.relayUrl,
        roomCode: req.roomCode,
        threshold: req.threshold ?? 2,
        maxSigners: req.maxSigners ?? 3,
        labelPrefix: req.labelPrefix ?? 'POKER',
        hide: req.hide ?? true,
      },
      (resp: any) => {
        if (chrome.runtime.lastError) {
          resolve({ success: false, error: chrome.runtime.lastError.message ?? 'sendMessage failed' })
          return
        }
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

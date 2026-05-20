// poker escrow DKG via zafu's generic zafu_dkg_join API.

export interface PokerDkgRequest {
  relayUrl: string
  roomCode: string
  threshold?: number
  maxSigners?: number
  /** wallet label prefix; zafu appends "-YYYY-MM-DD-HHMM" */
  labelPrefix?: string
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

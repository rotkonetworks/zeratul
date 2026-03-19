/**
 * Service/Filter/Future composition for poker.
 *
 * Following Eriksen 2013 "Your Server as a Function":
 *   Service[Req, Rep] = Req => Future[Rep]
 *   Filter[Req, Rep]  = (Req, Service[Req, Rep]) => Future[Rep]
 *
 * A service is an async function. A filter wraps a service,
 * adding cross-cutting concerns (encryption, signing, tracing).
 * Filters compose via andThen to build pipelines.
 *
 * The poker message pipeline:
 *
 *   inbound:  wsReceive → decrypt → verify → route → gameLogic
 *   outbound: gameAction → sign → encrypt → wsSend
 *
 * Each arrow is a filter or service. Concerns are orthogonal.
 * The game engine is a pure Service[Action, Event[]].
 */

// ============================================================================
// Core types (Eriksen §3)
// ============================================================================

/** A service is an async function from Req to Rep. */
export type Service<Req, Rep> = (req: Req) => Promise<Rep>

/**
 * A filter transforms a service. It receives a request and
 * the next service in the chain, returning a future.
 *
 * Filter[ReqIn, RepOut, ReqOut, RepIn] but for simplicity
 * we use the common case where types don't change.
 */
export type Filter<Req, Rep> = (req: Req, service: Service<Req, Rep>) => Promise<Rep>

/** Compose a filter with a service, producing a new service. */
export function andThen<Req, Rep>(
  filter: Filter<Req, Rep>,
  service: Service<Req, Rep>,
): Service<Req, Rep> {
  return (req: Req) => filter(req, service)
}

/** Compose two filters, producing a new filter. */
export function compose<Req, Rep>(
  outer: Filter<Req, Rep>,
  inner: Filter<Req, Rep>,
): Filter<Req, Rep> {
  return (req: Req, service: Service<Req, Rep>) =>
    outer(req, (r: Req) => inner(r, service))
}

/** Chain multiple filters left-to-right, then apply to a service. */
export function pipeline<Req, Rep>(
  ...filters: Filter<Req, Rep>[]
): (service: Service<Req, Rep>) => Service<Req, Rep> {
  return (service: Service<Req, Rep>) =>
    filters.reduceRight(
      (svc, filter) => andThen(filter, svc),
      service,
    )
}

// ============================================================================
// Wire message types
// ============================================================================

export interface WireMsg {
  t: string
  d: unknown
}

/** Envelope with optional crypto metadata. */
export interface Envelope {
  msg: WireMsg
  /** sender's session public key (hex) */
  sender?: string
  /** ed25519 signature over the payload */
  sig?: string
  /** was this message encrypted in transit? */
  encrypted?: boolean
}

// ============================================================================
// Filters
// ============================================================================

/**
 * Encryption filter (Eriksen §3: application-independent concern).
 *
 * Outbound: encrypts the payload with AES-256-GCM session key.
 * Inbound:  decrypts before passing to the next service.
 */
export function encryptFilter(
  encrypt: (plain: string) => Promise<string>,
): Filter<Envelope, void> {
  return async (env, next) => {
    const plain = JSON.stringify(env.msg)
    const ct = await encrypt(plain)
    return next({ ...env, msg: { t: '_enc', d: { p: ct } }, encrypted: true })
  }
}

export function decryptFilter(
  decrypt: (ct: string) => Promise<string>,
): Filter<Envelope, void> {
  return async (env, next) => {
    if (env.msg.t === '_enc') {
      const plain = await decrypt((env.msg.d as any).p)
      const inner: WireMsg = JSON.parse(plain)
      return next({ ...env, msg: inner, encrypted: true })
    }
    return next(env)
  }
}

/**
 * Signing filter.
 *
 * Outbound: signs the serialized payload with session ed25519 key.
 * Inbound:  verifies signature against sender's public key.
 */
export function signFilter(
  sign: (data: Uint8Array) => Promise<string>,
  sessionPub: string,
): Filter<Envelope, void> {
  return async (env, next) => {
    const payload = JSON.stringify(env.msg)
    const sig = await sign(new TextEncoder().encode(payload))
    return next({ ...env, sig, sender: sessionPub })
  }
}

export function verifyFilter(
  verify: (data: Uint8Array, sig: string, pubkey: string) => Promise<boolean>,
): Filter<Envelope, void> {
  return async (env, next) => {
    if (env.sig && env.sender) {
      const payload = JSON.stringify(env.msg)
      const valid = await verify(new TextEncoder().encode(payload), env.sig, env.sender)
      if (!valid) {
        console.warn('[verify] invalid signature from', env.sender?.slice(0, 8))
        return // drop message (Eriksen: "short-circuit on failure")
      }
    }
    return next(env)
  }
}

/**
 * Logging filter (cf. Eriksen §4.3: recordHandletime, logRequest).
 */
export function logFilter(label: string): Filter<Envelope, void> {
  return async (env, next) => {
    const t0 = performance.now()
    const result = await next(env)
    const dt = (performance.now() - t0).toFixed(1)
    console.log(`[${label}] ${env.msg.t} (${dt}ms)`)
    return result
  }
}

/**
 * Timeout filter (Eriksen §3: timeoutFilter).
 */
export function timeoutFilter<Req, Rep>(ms: number): Filter<Req, Rep> {
  return (req, service) => {
    return Promise.race([
      service(req),
      new Promise<Rep>((_, reject) =>
        setTimeout(() => reject(new Error(`timeout after ${ms}ms`)), ms),
      ),
    ])
  }
}

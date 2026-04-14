/**
 * zid contacts — local contact store with app-scoped handles
 *
 * contacts live in localStorage, managed by zid independently.
 * when zafu is connected, contacts can be imported from the wallet.
 * handles are BLAKE2b(contact_pubkey || app_origin || "zid:contact:v1")
 * — deterministic per contact+app, unlinkable across apps.
 */

import type { ContactRef } from './types'

const STORAGE_KEY = 'zid_contacts'

/** internal contact record */
interface StoredContact {
  /** session pubkey of this contact */
  pubkey: string
  /** display name (user-chosen) */
  name: string
  /** when we first saw them */
  addedAt: number
  /** last interaction */
  lastSeenAt: number
  /** app-scoped handle (computed on first use per app) */
  handles: Record<string, string> // appOrigin → handle
}

/** compute app-scoped handle via SHA-256 (BLAKE2b not in Web Crypto, SHA-256 is fine) */
async function computeHandle(pubkey: string, appOrigin: string): Promise<string> {
  const input = new TextEncoder().encode(`${pubkey}:${appOrigin}:zid:contact:v1`)
  const hash = new Uint8Array(await crypto.subtle.digest('SHA-256', input))
  return Array.from(hash).map(b => b.toString(16).padStart(2, '0')).join('')
}

function loadContacts(): StoredContact[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return raw ? JSON.parse(raw) : []
  } catch {
    return []
  }
}

function saveContacts(contacts: StoredContact[]) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(contacts))
}

/** add or update a contact (called when you interact with someone) */
export function upsertContact(pubkey: string, name: string) {
  const contacts = loadContacts()
  const existing = contacts.find(c => c.pubkey === pubkey)
  if (existing) {
    existing.name = name
    existing.lastSeenAt = Date.now()
  } else {
    contacts.push({ pubkey, name, addedAt: Date.now(), lastSeenAt: Date.now(), handles: {} })
  }
  saveContacts(contacts)
}

/** remove a contact */
export function removeContact(pubkey: string) {
  const contacts = loadContacts().filter(c => c.pubkey !== pubkey)
  saveContacts(contacts)
}

/** get all contacts as ContactRefs for a given app */
export async function getContactRefs(appOrigin: string): Promise<ContactRef[]> {
  const contacts = loadContacts()
  const refs: ContactRef[] = []
  for (const c of contacts) {
    if (!c.handles[appOrigin]) {
      c.handles[appOrigin] = await computeHandle(c.pubkey, appOrigin)
    }
    refs.push({ handle: c.handles[appOrigin], displayName: c.name })
  }
  saveContacts(contacts) // persist computed handles
  return refs
}

/** resolve a handle back to a pubkey (only works within the same app) */
export function resolveHandle(handle: string, appOrigin: string): string | null {
  const contacts = loadContacts()
  for (const c of contacts) {
    if (c.handles[appOrigin] === handle) return c.pubkey
  }
  return null
}

/** get recent contacts (sorted by lastSeenAt) */
export async function getRecentContacts(appOrigin: string, limit = 10): Promise<ContactRef[]> {
  const contacts = loadContacts()
    .sort((a, b) => b.lastSeenAt - a.lastSeenAt)
    .slice(0, limit)
  const refs: ContactRef[] = []
  for (const c of contacts) {
    if (!c.handles[appOrigin]) {
      c.handles[appOrigin] = await computeHandle(c.pubkey, appOrigin)
    }
    refs.push({ handle: c.handles[appOrigin], displayName: c.name })
  }
  return refs
}

/** picker: show a simple prompt-based picker (real UI would be in-page component) */
export async function pickFromLocal(
  appOrigin: string,
  opts: { purpose?: string; max?: number } = {},
): Promise<ContactRef[]> {
  const all = await getContactRefs(appOrigin)
  if (all.length === 0) return []
  // in a real implementation this would open a modal/popup
  // for now, return all contacts (the app's UI handles selection)
  return all.slice(0, opts.max || 1)
}

/** import contacts from zafu wallet (if connected) */
export function importFromWallet(contacts: Array<{ pubkey: string; name: string }>) {
  for (const c of contacts) {
    upsertContact(c.pubkey, c.name)
  }
}

/** count of contacts */
export function contactCount(): number {
  return loadContacts().length
}

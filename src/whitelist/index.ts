import fs from 'fs';
import { PublicKey } from '@solana/web3.js';

/**
 * In-process launcher whitelist.
 *
 * Redis used to be queried (sIsMember + hGetAll) between detecting a launch and firing the
 * buy, which put a network round trip and two awaits on the hot path. The set now lives in
 * memory and is refreshed in the background; the hot-path check is synchronous.
 *
 * Keys are stored as latin1 strings of the raw 32 bytes: hashing those is far cheaper than
 * base58-encoding a pubkey on every launch.
 */
let allowed = new Set<string>();
let watching = false;

export const WHITELIST_PATH = process.env.WHITELIST_PATH ?? 'whitelist.txt';

export function keyOf(pubkey: Buffer | Uint8Array): string {
  return Buffer.from(pubkey).toString('latin1');
}

/** Hot path: synchronous, no allocation beyond the lookup key. */
export function isWhitelisted(pubkey: Buffer | Uint8Array): boolean {
  return allowed.has(keyOf(pubkey));
}

export function whitelistSize(): number {
  return allowed.size;
}

export function loadWhitelistSync(path: string = WHITELIST_PATH): number {
  let raw: string;
  try {
    raw = fs.readFileSync(path, 'utf8');
  } catch {
    return -1;
  }
  const next = new Set<string>();
  for (const line of raw.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith('#')) continue;
    try {
      next.add(keyOf(new PublicKey(trimmed).toBuffer()));
    } catch {
      // skip malformed lines rather than dropping the whole file
    }
  }
  allowed = next;
  return allowed.size;
}

/** Reloads whenever the file changes, plus a 5s poll in case the watch is missed. */
export function startWhitelistRefresh(path: string = WHITELIST_PATH) {
  if (watching) return;
  watching = true;
  loadWhitelistSync(path);
  try {
    fs.watch(path, { persistent: false }, () => loadWhitelistSync(path));
  } catch {
    // file may not exist yet; the poll below picks it up
  }
  setInterval(() => loadWhitelistSync(path), 5000).unref();
}

/** Mints already fired on, so a re-detected launch is not bought twice. */
const seenMints = new Set<string>();

export function markSeen(mint: Buffer | Uint8Array): boolean {
  const key = keyOf(mint);
  if (seenMints.has(key)) return false;
  seenMints.add(key);
  if (seenMints.size > 50_000) seenMints.clear();
  return true;
}

/**
 * Pure ladder-exit math, split out from the seller so it can be unit-tested without a chain.
 * Everything here is deterministic: no RPC, no wall clock, no wallet.
 */

/** Parse `SNIPER_EXIT_LEGS`-style "slot:bps,slot:bps" into [slotOffset, fraction] pairs. */
export function parseLegs(s?: string): Array<[number, number]> | undefined {
  if (!s) return undefined;
  const out: Array<[number, number]> = [];
  for (const part of s.split(',')) {
    const [slot, bps] = part.split(':');
    const sl = Number(slot);
    const fr = Number(bps) / 10_000;
    if (!Number.isFinite(sl) || !Number.isFinite(fr)) return undefined;
    out.push([sl, fr]);
  }
  return out.length ? out : undefined;
}

/** bonding-curve reserves: virtualTokenReserves @ 8..16, virtualSolReserves @ 16..24 (LE u64).
 *  Matches the Rust reader in position.rs. */
export function readReserves(data: Buffer): { vToken: bigint; vSol: bigint } | null {
  if (data.length < 24) return null;
  return { vToken: data.readBigUInt64LE(8), vSol: data.readBigUInt64LE(16) };
}

/** The creator pubkey lives at byte 49 of the bonding-curve account (disc8 + 5×u64 + complete1).
 *  Reconciliation reads it to build the sell (creator_vault PDA); a wrong offset = wrong vault =
 *  failed sell, so the constant is pinned by a test. Returns the 32 raw bytes, or null for the
 *  pre-creator-field layout (too short to carry one). */
export function readCreatorBytes(data: Buffer): Buffer | null {
  if (data.length < 81) return null;
  return data.subarray(49, 81);
}

/** Lamports out for selling `tokens` into the curve, fee deducted (constant product). */
export function sellValue(vSol: bigint, vToken: bigint, tokens: bigint, feeBps = 125): bigint {
  if (tokens <= 0n || vToken <= 0n || vSol <= 0n) return 0n;
  const k = vSol * vToken;
  const grossOut = vSol - k / (vToken + tokens);
  return (grossOut * BigInt(10_000 - feeBps)) / 10_000n;
}

/**
 * How much a ladder leg sells, and whether it is the final leg. `frac` is the fraction of the
 * ORIGINAL position (null = force-out, sell everything). Price overrides on the current value
 * multiple: <= stopX dumps all now; >= moonX trims only moonFrac and keeps riding. Never sells
 * more than `remaining`, and flags the leg final when the remainder would be dust.
 *
 * This is the money decision, isolated: given position sizes and a price multiple, exactly how
 * many tokens leave and does the account close.
 */
export function decideLegSize(
  original: bigint,
  remaining: bigint,
  frac: number | null,
  multiple: number | null,
  opts: { stopX: number; moonX: number; moonFrac: number; dust: bigint },
): { sellTokens: bigint; isFinal: boolean } {
  let dumpAll = frac === null;
  if (multiple !== null && multiple <= opts.stopX) dumpAll = true;

  let sellTokens: bigint;
  if (dumpAll) {
    sellTokens = remaining;
  } else if (frac === 0) {
    // a zero-fraction leg is a pure stop-check (used to build stop8 out of the ladder): it
    // sells nothing unless the stop above fired. It must NOT moon-trim either.
    sellTokens = 0n;
  } else if (multiple !== null && multiple >= opts.moonX) {
    sellTokens = (original * BigInt(Math.round(opts.moonFrac * 10_000))) / 10_000n;
  } else {
    sellTokens = (original * BigInt(Math.round((frac as number) * 10_000))) / 10_000n;
  }
  if (sellTokens > remaining) sellTokens = remaining;
  if (sellTokens < 0n) sellTokens = 0n;
  const isFinal = dumpAll || remaining - sellTokens <= opts.dust;
  return { sellTokens, isFinal };
}

/** Value multiple of the remaining position vs its cost basis; null when inputs are unusable. */
export function valueMultiple(
  vSol: bigint,
  vToken: bigint,
  remaining: bigint,
  original: bigint,
  entryCostLamports: bigint,
  feeBps = 125,
): number | null {
  if (original <= 0n || entryCostLamports <= 0n) return null;
  const value = sellValue(vSol, vToken, remaining, feeBps);
  const costForRemaining = (entryCostLamports * remaining) / original;
  if (costForRemaining <= 0n) return null;
  return Number(value) / Number(costForRemaining);
}

import { describe, it, expect } from 'vitest';
import { parseLegs, readReserves, sellValue, valueMultiple, decideLegSize, readCreatorBytes } from './ladder';

const SOL = 1_000_000_000n;
const opts = { stopX: 0.8, moonX: 2.0, moonFrac: 0.05, dust: 1000n };

describe('parseLegs', () => {
  it('parses slot:bps pairs into [slot, fraction]', () => {
    expect(parseLegs('1:3000,6:2000,18:1500')).toEqual([
      [1, 0.3],
      [6, 0.2],
      [18, 0.15],
    ]);
  });
  it('returns undefined for empty or missing input', () => {
    expect(parseLegs(undefined)).toBeUndefined();
    expect(parseLegs('')).toBeUndefined();
  });
  it('returns undefined on malformed input rather than a bad ladder', () => {
    expect(parseLegs('1-3000')).toBeUndefined();
    expect(parseLegs('x:y')).toBeUndefined();
  });
});

describe('readReserves', () => {
  it('reads vToken @8..16 and vSol @16..24, matching the Rust reader', () => {
    const buf = Buffer.alloc(81);
    buf.writeBigUInt64LE(1_000_000_000_000_000n, 8); // vToken
    buf.writeBigUInt64LE(35_000_000_000n, 16); // vSol
    const r = readReserves(buf)!;
    expect(r.vToken).toBe(1_000_000_000_000_000n);
    expect(r.vSol).toBe(35_000_000_000n);
  });
  it('returns null for a truncated account', () => {
    expect(readReserves(Buffer.alloc(20))).toBeNull();
  });
});

describe('readCreatorBytes (orphan reconciliation)', () => {
  it('reads the 32-byte creator at offset 49', () => {
    const buf = Buffer.alloc(81);
    const creator = Buffer.alloc(32, 7);
    creator.copy(buf, 49);
    expect(readCreatorBytes(buf)).toEqual(creator);
  });
  it('returns null for the pre-creator (short) layout rather than a garbage vault', () => {
    expect(readCreatorBytes(Buffer.alloc(48))).toBeNull();
    expect(readCreatorBytes(Buffer.alloc(80))).toBeNull();
  });
});

describe('sellValue (constant product)', () => {
  const vToken = 1_000_000_000_000_000n;
  const vSol = 35_000_000_000n;
  it('is zero for non-positive inputs', () => {
    expect(sellValue(vSol, vToken, 0n)).toBe(0n);
    expect(sellValue(0n, vToken, 100n)).toBe(0n);
    expect(sellValue(vSol, 0n, 100n)).toBe(0n);
  });
  it('moves price down: selling more tokens yields more SOL but at a worse marginal rate', () => {
    const small = sellValue(vSol, vToken, 10_000_000_000n);
    const big = sellValue(vSol, vToken, 20_000_000_000n);
    expect(big).toBeGreaterThan(small);
    // marginal: doubling tokens sold yields LESS than double SOL (price impact)
    expect(big).toBeLessThan(small * 2n);
  });
  it('applies the fee: net is below the gross curve delta', () => {
    const tokens = 10_000_000_000n;
    const k = vSol * vToken;
    const gross = vSol - k / (vToken + tokens);
    const net = sellValue(vSol, vToken, tokens, 125);
    expect(net).toBeLessThan(gross);
    expect(net).toBe((gross * 9875n) / 10_000n);
  });
});

describe('valueMultiple', () => {
  const vToken = 1_000_000_000_000_000n;
  const vSol = 35_000_000_000n;
  it('returns null when cost basis or original is missing (orphan force-out)', () => {
    expect(valueMultiple(vSol, vToken, SOL, 0n, SOL)).toBeNull();
    expect(valueMultiple(vSol, vToken, SOL, SOL, 0n)).toBeNull();
  });
  it('is ~1 when the current sell value equals the cost basis', () => {
    const tokens = 5_000_000_000n;
    const value = sellValue(vSol, vToken, tokens);
    const m = valueMultiple(vSol, vToken, tokens, tokens, value)!;
    expect(m).toBeCloseTo(1.0, 6);
  });
  it('is >1 when the curve has risen above entry', () => {
    const tokens = 5_000_000_000n;
    const cost = sellValue(vSol, vToken, tokens);
    const higher = valueMultiple(vSol * 2n, vToken, tokens, tokens, cost)!;
    expect(higher).toBeGreaterThan(1.0);
  });
});

/**
 * Regression for the exact_sol_in cost-basis P0.
 *
 * The proxy's Fill.max_sol_cost field MUST carry the priced budget in lamports for both buy
 * instructions (pinned on the Rust side by forwarder::fill_carries_cost_basis_and_
 * expected_tokens_not_the_wire_args). These tests pin the seller-side consequence with
 * realistic magnitudes: a lamport cost basis prices sanely, while the raw exact_sol_in wire
 * arg (a token floor, ~1e13) collapses the multiple to ~0 and trips the 0.8x stop on a
 * healthy position - which is exactly the bug (every position dumped at slot +1).
 */
describe('cost basis units (exact_sol_in regression)', () => {
  // realistic post-entry curve: ~2 SOL of prior flow + our 2 SOL buy
  const vSol = 34_000_000_000n;
  const vToken = (30_000_000_000n * 1_073_000_000_000_000n) / vSol;
  const budgetLamports = 2_000_000_000n; // what the proxy now sends
  const tokenFloor = 60_000_000_000_000n; // what the raw exact_sol_in wire arg looks like
  const held = 63_000_000_000_000n; // ~2 SOL worth of tokens at this depth

  it('a lamport cost basis prices a healthy position near 1x (no stop)', () => {
    const m = valueMultiple(vSol, vToken, held, held, budgetLamports)!;
    expect(m).toBeGreaterThan(0.8); // must NOT trip the stop
    expect(m).toBeLessThan(1.2);
    const { sellTokens, isFinal } = decideLegSize(held, held, 0.3, m, opts);
    expect(isFinal).toBe(false); // scheduled leg, not a dump
    expect(sellTokens).toBe((held * 3n) / 10n);
  });

  it('a token-unit cost basis reads ~0x and would dump everything at leg 1 - the bug', () => {
    const m = valueMultiple(vSol, vToken, held, held, tokenFloor)!;
    expect(m).toBeLessThan(0.001); // ~10,000x off
    const { sellTokens, isFinal } = decideLegSize(held, held, 0.3, m, opts);
    expect(isFinal).toBe(true); // stop fires, whole bag gone at slot +1
    expect(sellTokens).toBe(held);
  });
});

describe('decideLegSize — the money decision', () => {
  const original = 1_000_000n;
  it('sells the scheduled fraction of ORIGINAL on a normal leg', () => {
    const { sellTokens, isFinal } = decideLegSize(original, original, 0.3, 1.1, opts);
    expect(sellTokens).toBe(300_000n);
    expect(isFinal).toBe(false);
  });
  it('force-out (frac null) sells everything remaining and is final', () => {
    const { sellTokens, isFinal } = decideLegSize(original, 400_000n, null, 1.1, opts);
    expect(sellTokens).toBe(400_000n);
    expect(isFinal).toBe(true);
  });
  it('final leg sells the FULL remainder, never leaving rounding dust (live regression)', () => {
    // mint 2j6sw4Wy… 2026-08-26: odd remainder 3737163854755, 15% leg floored to …754,
    // leaving 1 raw token — the account close reverted the whole sell (Token-2022 error 11).
    const odd = 3_737_163_854_755n;
    const { sellTokens, isFinal } = decideLegSize(24_914_425_698_361n, odd, 0.15, 1.1, opts);
    expect(isFinal).toBe(true);
    expect(sellTokens).toBe(odd); // everything, not the floored fraction
  });
  it('STOP: multiple <= stopX dumps all remaining now, marked final', () => {
    const { sellTokens, isFinal } = decideLegSize(original, 700_000n, 0.2, 0.75, opts);
    expect(sellTokens).toBe(700_000n);
    expect(isFinal).toBe(true);
  });
  it('MOON: multiple >= moonX trims only moonFrac and keeps riding', () => {
    const { sellTokens, isFinal } = decideLegSize(original, original, 0.3, 2.5, opts);
    expect(sellTokens).toBe(50_000n); // 5% of original, not the 30% leg
    expect(isFinal).toBe(false);
  });
  it('never sells more than remaining, and that is final', () => {
    const { sellTokens, isFinal } = decideLegSize(original, 100_000n, 0.3, 1.1, opts);
    expect(sellTokens).toBe(100_000n); // 30% of 1_000_000 = 300k, clamped to 100k held
    expect(isFinal).toBe(true);
  });
  it('flags final when the remainder after the leg would be dust', () => {
    const { isFinal } = decideLegSize(original, 300_500n, 0.3, 1.1, opts);
    // sells 300k, leaves 500 <= dust(1000) -> final
    expect(isFinal).toBe(true);
  });
  it('a zero-fraction leg sells nothing (pure stop-check for stop8), even on a moon', () => {
    const normal = decideLegSize(original, original, 0, 1.1, opts);
    expect(normal.sellTokens).toBe(0n);
    const mooned = decideLegSize(original, original, 0, 2.5, opts);
    expect(mooned.sellTokens).toBe(0n); // must NOT trim 5% on a check-only leg
    const crashed = decideLegSize(original, original, 0, 0.7, opts);
    expect(crashed.sellTokens).toBe(original); // but a stop still dumps all
    expect(crashed.isFinal).toBe(true);
  });

  it('a null multiple (unreadable curve) falls through to the scheduled fraction', () => {
    const { sellTokens, isFinal } = decideLegSize(original, original, 0.2, null, opts);
    expect(sellTokens).toBe(200_000n);
    expect(isFinal).toBe(false);
  });
  it('stop takes priority over moon when both could apply is impossible, but stop beats a normal leg', () => {
    const crashed = decideLegSize(original, original, 0.15, 0.5, opts);
    expect(crashed.sellTokens).toBe(original); // dumped all
    expect(crashed.isFinal).toBe(true);
  });
});

import { decideLegSizeCurve } from './ladder';
describe('decideLegSizeCurve — E4Ez measured price-conditional exit', () => {
  const O = 1_000_000n;
  const opts = { firstFrac: 0.30, stopFrac: 0.55, dust: 1000n };
  it('first leg sells 30% unconditionally (lock-in)', () => {
    const r = decideLegSizeCurve(O, O, 5.0, true, false, opts); // even mooning, first leg = 30%
    expect(r.sellTokens).toBe(300_000n);
  });
  it('losing (<=0.8x): partial 55% of REMAINING, not a full dump, not final', () => {
    const r = decideLegSizeCurve(O, 700_000n, 0.7, false, false, opts);
    expect(r.sellTokens).toBe((700_000n * 5500n) / 10_000n); // 55% of remaining
    expect(r.isFinal).toBe(false); // keeps 45% for a bounce
  });
  it('1-1.5x trims 17.5%, 1.5-2x trims 4%, >=2x trims 1.8% (rides winners)', () => {
    expect(decideLegSizeCurve(O, O, 1.2, false, false, opts).sellTokens).toBe(175_000n);
    expect(decideLegSizeCurve(O, O, 1.7, false, false, opts).sellTokens).toBe(40_000n);
    expect(decideLegSizeCurve(O, O, 3.0, false, false, opts).sellTokens).toBe(18_000n);
  });
  it('force-out sells the full remainder and is final', () => {
    const r = decideLegSizeCurve(O, 250_000n, 2.5, false, true, opts);
    expect(r.sellTokens).toBe(250_000n);
    expect(r.isFinal).toBe(true);
  });
  it('never leaves dust (Token-2022 close needs empty)', () => {
    const r = decideLegSizeCurve(O, 900n, 1.2, false, false, opts);
    expect(r.isFinal).toBe(true);
    expect(r.sellTokens).toBe(900n);
  });
});

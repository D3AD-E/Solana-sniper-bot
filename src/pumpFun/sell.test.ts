import { describe, expect, it } from 'vitest';
import { PublicKey } from '@solana/web3.js';
import { sellInstruction, Position } from './sell';
import { PumpGlobal } from './global';
import {
  GLOBAL,
  PUMP_PROGRAM,
  EVENT_AUTHORITY,
  FEE_CONFIG,
  FEE_PROGRAM,
  SYSTEM_PROGRAM,
  SEED_CREATOR_VAULT,
  SEED_BONDING_CURVE_V2,
  SEED_USER_VOLUME_ACCUMULATOR,
} from './constants';

/**
 * The sell's remaining accounts are CONDITIONAL on the launch type, verified against real
 * successful sells on 2026-08-26:
 *  - cashback launch  (mint CXkQajJZ…, tx 3N7Qodr…): [..14 IDL, user_volume_accumulator,
 *    bonding_curve_v2, buyback]  — 17 accounts. Missing accumulator => 6073.
 *  - plain/buyback    (mint BxPwpob5…, tx 2eCGyjeB…): [..14 IDL, bonding_curve_v2, buyback]
 *    — 16 accounts. Present accumulator => 6074.
 * Both mistakes trapped live bags; these tests pin the exact orders.
 */
describe('sellInstruction remaining accounts', () => {
  const seller = PublicKey.unique();
  const global: PumpGlobal = {
    feeRecipient: PublicKey.unique(),
    buybackFeeRecipients: [PublicKey.unique()],
  } as unknown as PumpGlobal;
  const base = {
    mint: PublicKey.unique(),
    tokenAccount: PublicKey.unique(),
    seed: 's',
    tokenProgram: PublicKey.unique(),
    bondingCurve: PublicKey.unique(),
    associatedBondingCurve: PublicKey.unique(),
    creator: PublicKey.unique(),
  };
  const pda = (seeds: Buffer[]) => PublicKey.findProgramAddressSync(seeds, PUMP_PROGRAM)[0];

  const idl14 = (p: Position) => [
    GLOBAL,
    global.feeRecipient,
    p.mint,
    p.bondingCurve,
    p.associatedBondingCurve,
    p.tokenAccount,
    seller,
    SYSTEM_PROGRAM,
    pda([SEED_CREATOR_VAULT, p.creator.toBuffer()]),
    p.tokenProgram,
    EVENT_AUTHORITY,
    PUMP_PROGRAM,
    FEE_CONFIG,
    FEE_PROGRAM,
  ];

  it('plain launch: 16 accounts, [bcv2, buyback] — no accumulator (BxPw layout)', () => {
    const p: Position = { ...base, cashback: false };
    const keys = sellInstruction(global, seller, p, 1n, 0n).keys.map((k) => k.pubkey.toBase58());
    const want = [
      ...idl14(p),
      pda([SEED_BONDING_CURVE_V2, p.mint.toBuffer()]),
      global.buybackFeeRecipients[0],
    ].map((k) => k.toBase58());
    expect(keys).toEqual(want);
    expect(keys).toHaveLength(16);
  });

  it('cashback launch: 17 accounts, [accumulator, bcv2, buyback] (CXkQ layout)', () => {
    const p: Position = { ...base, cashback: true };
    const keys = sellInstruction(global, seller, p, 1n, 0n).keys.map((k) => k.pubkey.toBase58());
    const want = [
      ...idl14(p),
      pda([SEED_USER_VOLUME_ACCUMULATOR, seller.toBuffer()]),
      pda([SEED_BONDING_CURVE_V2, p.mint.toBuffer()]),
      global.buybackFeeRecipients[0],
    ].map((k) => k.toBase58());
    expect(keys).toEqual(want);
    expect(keys).toHaveLength(17);
  });
});

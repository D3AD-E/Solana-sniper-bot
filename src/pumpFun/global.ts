import { Connection, PublicKey } from '@solana/web3.js';
import { GLOBAL } from './constants';

export type PumpGlobal = {
  feeRecipient: PublicKey;
  buybackFeeRecipients: PublicKey[];
  initialVirtualSolReserves: bigint;
  initialVirtualTokenReserves: bigint;
  initialRealTokenReserves: bigint;
};

/**
 * Reads the deployed pump.fun `Global` account. The layout gained several fields when
 * `create_v2`, mayhem mode and the buyback recipients shipped, so the values that matter to
 * a buy (fee recipient, buyback recipient, initial reserves) are read straight from chain
 * instead of being hardcoded or taken from an outdated SDK.
 */
export async function fetchPumpGlobal(connection: Connection): Promise<PumpGlobal> {
  const info = await connection.getAccountInfo(GLOBAL, 'confirmed');
  if (!info) throw new Error('pump.fun global account not found');
  const d = info.data;
  let o = 8;

  const pk = () => {
    const v = new PublicKey(d.subarray(o, o + 32));
    o += 32;
    return v;
  };
  const u64 = () => {
    const v = d.readBigUInt64LE(o);
    o += 8;
    return v;
  };
  const bool = () => {
    o += 1;
  };

  bool(); // initialized
  pk(); // authority
  const feeRecipient = pk();
  const initialVirtualTokenReserves = u64();
  const initialVirtualSolReserves = u64();
  const initialRealTokenReserves = u64();
  u64(); // token_total_supply
  u64(); // fee_basis_points
  pk(); // withdraw_authority
  bool(); // enable_migrate
  u64(); // pool_migration_fee
  u64(); // creator_fee_basis_points
  for (let i = 0; i < 7; i++) pk(); // fee_recipients
  pk(); // set_creator_authority
  pk(); // admin_set_creator_authority
  bool(); // create_v2_enabled
  pk(); // whitelist_pda
  pk(); // reserved_fee_recipient
  bool(); // mayhem_mode_enabled
  for (let i = 0; i < 7; i++) pk(); // reserved_fee_recipients
  bool(); // is_cashback_enabled
  const buybackFeeRecipients: PublicKey[] = [];
  for (let i = 0; i < 8; i++) buybackFeeRecipients.push(pk());

  return {
    feeRecipient,
    buybackFeeRecipients,
    initialVirtualSolReserves,
    initialVirtualTokenReserves,
    initialRealTokenReserves,
  };
}

/** Tokens received for `solIn` lamports on a fresh curve. */
export function initialBuyPrice(global: PumpGlobal, solIn: bigint): bigint {
  if (solIn <= 0n) return 0n;
  const n = global.initialVirtualSolReserves * global.initialVirtualTokenReserves;
  const i = global.initialVirtualSolReserves + solIn;
  const r = n / i + 1n;
  const s = global.initialVirtualTokenReserves - r;
  return s < global.initialRealTokenReserves ? s : global.initialRealTokenReserves;
}

/** Tokens we expect when buying `ourSol` right behind a dev buy of `devSol`. */
export function tokensBehindDevBuy(global: PumpGlobal, devSol: bigint, ourSol: bigint): bigint {
  const before = initialBuyPrice(global, devSol);
  const after = initialBuyPrice(global, devSol + ourSol);
  const out = after - before - 500n;
  return out > 0n ? out : 0n;
}

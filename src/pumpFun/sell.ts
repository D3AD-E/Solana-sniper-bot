import {
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionInstruction,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import {
  EVENT_AUTHORITY,
  FEE_CONFIG,
  FEE_PROGRAM,
  GLOBAL,
  PUMP_PROGRAM,
  SEED_BONDING_CURVE_V2,
  SEED_CREATOR_VAULT,
  SEED_USER_VOLUME_ACCUMULATOR,
  SYSTEM_PROGRAM,
} from './constants';
import { PumpGlobal } from './global';

/** anchor discriminator for pump.fun `sell` */
const DISC_SELL = Buffer.from([51, 230, 133, 164, 1, 127, 131, 173]);
/** spl-token / token-2022 CloseAccount */
const TOKEN_IX_CLOSE_ACCOUNT = 9;

export const SELL_COMPUTE_UNIT_LIMIT = 110_000;

const ro = (pubkey: PublicKey) => ({ pubkey, isSigner: false, isWritable: false });
const rw = (pubkey: PublicKey) => ({ pubkey, isSigner: false, isWritable: true });

function u64le(value: bigint): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(value);
  return b;
}

function pda(seeds: Buffer[], programId: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync(seeds, programId)[0];
}

export type Position = {
  mint: PublicKey;
  /** created from a seed by the buy, so it is not an ATA and cannot be re-derived */
  tokenAccount: PublicKey;
  seed: string;
  tokenProgram: PublicKey;
  bondingCurve: PublicKey;
  associatedBondingCurve: PublicKey;
  creator: PublicKey;
};

/**
 * pump.fun `sell`, built by hand.
 *
 * The deployed program takes 14 IDL accounts plus three remaining accounts:
 * `user_volume_accumulator`, `bonding_curve_v2` and a buyback fee recipient. Note the order
 * differs from `buy`: `creator_vault` comes before `token_program` here, and there is no
 * `global_volume_accumulator`.
 */
export function sellInstruction(
  global: PumpGlobal,
  seller: PublicKey,
  position: Position,
  amount: bigint,
  minSolOutput: bigint,
): TransactionInstruction {
  const creatorVault = pda([SEED_CREATOR_VAULT, position.creator.toBuffer()], PUMP_PROGRAM);
  const bondingCurveV2 = pda([SEED_BONDING_CURVE_V2, position.mint.toBuffer()], PUMP_PROGRAM);
  const userVolumeAccumulator = pda([SEED_USER_VOLUME_ACCUMULATOR, seller.toBuffer()], PUMP_PROGRAM);

  return new TransactionInstruction({
    programId: PUMP_PROGRAM,
    keys: [
      ro(GLOBAL),
      rw(global.feeRecipient),
      ro(position.mint),
      rw(position.bondingCurve),
      rw(position.associatedBondingCurve),
      rw(position.tokenAccount),
      { pubkey: seller, isSigner: true, isWritable: true },
      ro(SYSTEM_PROGRAM),
      rw(creatorVault),
      ro(position.tokenProgram),
      ro(EVENT_AUTHORITY),
      ro(PUMP_PROGRAM),
      ro(FEE_CONFIG),
      ro(FEE_PROGRAM),
      rw(userVolumeAccumulator),
      rw(bondingCurveV2),
      rw(global.buybackFeeRecipients[0]),
    ],
    data: Buffer.concat([DISC_SELL, u64le(amount), u64le(minSolOutput)]),
  });
}

/** Closes the seeded token account and returns its rent to the seller. */
export function closeAccountInstruction(position: Position, seller: PublicKey): TransactionInstruction {
  return new TransactionInstruction({
    programId: position.tokenProgram,
    keys: [rw(position.tokenAccount), rw(seller), ro(seller)],
    data: Buffer.from([TOKEN_IX_CLOSE_ACCOUNT]),
  });
}

/**
 * Builds and sends a sell.
 *
 * This side is deliberately unoptimized: it uses web3.js, a fresh blockhash and normal
 * signing rather than the byte-patched template the buy path uses. Selling happens seconds
 * after the buy, so the microseconds do not matter, and readability does.
 */
export async function sellPosition(
  connection: Connection,
  wallet: Keypair,
  global: PumpGlobal,
  position: Position,
  amount: bigint,
  options: { cuPrice?: bigint; tipAccount?: PublicKey; tipLamports?: bigint; minSolOutput?: bigint } = {},
): Promise<string> {
  const instructions: TransactionInstruction[] = [
    ComputeBudgetProgram.setComputeUnitLimit({ units: SELL_COMPUTE_UNIT_LIMIT }),
    ComputeBudgetProgram.setComputeUnitPrice({ microLamports: options.cuPrice ?? 100_000n }),
    sellInstruction(global, wallet.publicKey, position, amount, options.minSolOutput ?? 0n),
    closeAccountInstruction(position, wallet.publicKey),
  ];

  if (options.tipAccount && options.tipLamports) {
    instructions.push(
      SystemProgram.transfer({
        fromPubkey: wallet.publicKey,
        toPubkey: options.tipAccount,
        lamports: options.tipLamports,
      }),
    );
  }

  const { blockhash } = await connection.getLatestBlockhash('confirmed');
  const message = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: blockhash,
    instructions,
  }).compileToV0Message();

  const transaction = new VersionedTransaction(message);
  transaction.sign([wallet]);
  return connection.sendTransaction(transaction, { skipPreflight: true, maxRetries: 0 });
}

/** Current token balance of a position, 0 when the account is gone. */
export async function positionBalance(connection: Connection, position: Position): Promise<bigint> {
  const info = await connection.getTokenAccountBalance(position.tokenAccount, 'processed').catch(() => null);
  if (!info) return 0n;
  return BigInt(info.value.amount);
}

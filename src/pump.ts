import { Commitment, LAMPORTS_PER_SOL, PublicKey } from '@solana/web3.js';
import { credentials } from '@grpc/grpc-js';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { sendMessage } from './telegramBot';
import eventEmitter from './eventEmitter';
import { USER_STOP_EVENT } from './eventEmitter/eventEmitter.consts';
import { ShredstreamProxyClient } from './generated/shredstream/shredstream_grpc_pb';
import { SubscribeFillsRequest, Fill } from './generated/shredstream/shredstream_pb';
import { fetchPumpGlobal, PumpGlobal, Position, positionBalance, sellPosition } from './pumpFun';

/**
 * Node process: sells, telemetry, telegram.
 *
 * It never touches the buy path. The Rust proxy detects launches on the shred stream and
 * fires the buy itself, then publishes what it bought on a non-hot gRPC stream. This process
 * picks those up and manages the exit over a plain websocket connection.
 */

type OpenPosition = Position & {
  slot: number;
  boughtAt: Date;
  amount: bigint;
  selling: boolean;
};

const positions = new Map<string, OpenPosition>();
let pumpGlobal: PumpGlobal | undefined;
let softExit = false;
let initialWalletBalance = 0;

/** how long to hold before selling, in ms */
const HOLD_MS = Number(process.env.HOLD_MS ?? 2100);
/** shredstream proxy gRPC port, the same one the sniper serves */
const PROXY_PORT = process.env.SHREDSTREAM_GRPC_PORT ?? '9999';

eventEmitter.on(USER_STOP_EVENT, () => {
  softExit = true;
});

export default async function snipe(isMinimalRun: boolean = false): Promise<void> {
  pumpGlobal = await fetchPumpGlobal(solanaConnection);
  logger.info(`pump global: fee recipient ${pumpGlobal.feeRecipient.toBase58()}`);

  const balance = await solanaConnection.getBalance(wallet.publicKey);
  initialWalletBalance = balance / LAMPORTS_PER_SOL;
  logger.info(`Wallet balance (in SOL): ${initialWalletBalance}`);
  sendMessage(`Seller started${isMinimalRun ? ' (minimal)' : ''}`);

  subscribeToFills(PROXY_PORT);
}

/**
 * Buys the Rust sniper landed. The token account is created from a seed rather than being an
 * ATA, so the seed comes over the wire: it cannot be re-derived from the mint alone.
 */
function subscribeToFills(port: string) {
  const client = new ShredstreamProxyClient(`localhost:${port}`, credentials.createInsecure());
  const stream = client.subscribeFills(new SubscribeFillsRequest());

  stream.on('error', (err) => {
    logger.warn(`fills stream error: ${err.message}`);
    setTimeout(() => subscribeToFills(port), 1000);
  });

  stream.on('data', (fill: Fill) => {
    const mint = new PublicKey(fill.getMint_asU8());
    const key = mint.toBase58();
    if (positions.has(key)) return;

    const position: OpenPosition = {
      mint,
      tokenAccount: new PublicKey(fill.getTokenAccount_asU8()),
      seed: fill.getSeed(),
      tokenProgram: new PublicKey(fill.getTokenProgram_asU8()),
      bondingCurve: new PublicKey(fill.getBondingCurve_asU8()),
      associatedBondingCurve: new PublicKey(fill.getAssociatedBondingCurve_asU8()),
      creator: new PublicKey(fill.getCreator_asU8()),
      slot: fill.getSlot(),
      boughtAt: new Date(),
      amount: BigInt(fill.getAmount()),
      selling: false,
    };
    positions.set(key, position);

    logger.info(`bought ${key} slot ${position.slot} seed ${position.seed}`);
    sendMessage(`Bought ${key}`);
    setTimeout(() => exitPosition(key), HOLD_MS);
  });
}

async function exitPosition(key: string) {
  const position = positions.get(key);
  if (!position || position.selling) return;
  position.selling = true;

  try {
    // the buy may not have landed: the balance is the source of truth, not the fill
    let balance = await positionBalance(solanaConnection, position);
    if (balance === 0n) {
      await new Promise((resolve) => setTimeout(resolve, 1200));
      balance = await positionBalance(solanaConnection, position);
    }
    if (balance === 0n) {
      logger.info(`${key}: nothing to sell, buy did not land`);
      positions.delete(key);
      return;
    }

    const signature = await sellPosition(solanaConnection, wallet, pumpGlobal!, position, balance);
    logger.info(`sold ${key} amount ${balance} https://solscan.io/tx/${signature}`);
    sendMessage(`Sold ${key}`);

    // give the sell a moment, then confirm the account is gone before forgetting it
    await new Promise((resolve) => setTimeout(resolve, 5000));
    const remaining = await positionBalance(solanaConnection, position);
    if (remaining > 0n) {
      logger.warn(`${key}: ${remaining} tokens left, retrying`);
      position.selling = false;
      setTimeout(() => exitPosition(key), 1000);
      return;
    }
    positions.delete(key);
    await reportBalance();
  } catch (e) {
    logger.warn(`${key}: sell failed: ${(e as Error).message}`);
    position.selling = false;
    if (!softExit) setTimeout(() => exitPosition(key), 1500);
  }
}

async function reportBalance() {
  const balance = await solanaConnection.getBalance(wallet.publicKey, 'confirmed' as Commitment);
  const current = balance / LAMPORTS_PER_SOL;
  const diff = current - initialWalletBalance;
  logger.info(`${diff > 0 ? 'Trade won' : 'Trade loss'} diff ${diff.toFixed(4)} balance ${current.toFixed(4)}`);
  initialWalletBalance = current;
}

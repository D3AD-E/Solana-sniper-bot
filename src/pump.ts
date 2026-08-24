import { Commitment, LAMPORTS_PER_SOL, PublicKey } from '@solana/web3.js';
import { credentials } from '@grpc/grpc-js';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { sendMessage } from './telegramBot';
import eventEmitter from './eventEmitter';
import { USER_STOP_EVENT } from './eventEmitter/eventEmitter.consts';
import { ShredstreamProxyClient } from './generated/shredstream/shredstream_grpc_pb';
import { SubscribeFillsRequest, Fill } from './generated/shredstream/shredstream_pb';
import { fetchPumpGlobal, PumpGlobal, Position, sellPosition, amountFromAccountData } from './pumpFun';
import { sendTransactionHeliusSender, HELIUS_SENDER_TIP_ACCOUNTS } from './keepAliveHttp/healthCheck';

/**
 * Node process: sells, telemetry, telegram. It never touches the buy path.
 *
 * The Rust proxy buys and publishes what it bought on a non-hot gRPC stream. That message is
 * needed rather than optional: the buy creates its token account with createAccountWithSeed,
 * so the address cannot be derived from the mint the way an ATA could.
 *
 * The sell is triggered by the buy *confirming*, not by a wall clock. A websocket
 * subscription on that exact token account fires the moment the buy lands and carries the
 * real balance, so there is nothing to poll and no guessing whether the buy made it.
 */

type OpenPosition = Position & {
  slot: number;
  detectedAt: number;
  confirmedAt?: number;
  subscription?: number;
  selling: boolean;
  timer?: NodeJS.Timeout;
};

const positions = new Map<string, OpenPosition>();
let pumpGlobal: PumpGlobal | undefined;
let softExit = false;
let initialWalletBalance = 0;

/**
 * How long to hold after the buy confirms.
 *
 * Measured against a wallet running this strategy (24678QKx…, 72 positions): the first and
 * only sell lands 2 slots minimum, 6 slots median, 8 slots at p75 after the buy — about two
 * seconds — and it always dumps the whole balance in one transaction rather than laddering.
 * Since our trigger is confirmation rather than detection, the wait here is measured from
 * the moment the tokens actually appear.
 */
const HOLD_AFTER_CONFIRM_MS = Number(process.env.SELL_HOLD_MS ?? 1600);
/** give up on a position whose buy never landed */
const BUY_TIMEOUT_MS = Number(process.env.BUY_TIMEOUT_MS ?? 30_000);
const SELL_CU_PRICE = BigInt(process.env.SELL_CU_PRICE ?? 100_000);
const SELL_TIP_LAMPORTS = BigInt(process.env.SELL_TIP_LAMPORTS ?? 1_000_000);
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
      detectedAt: Date.now(),
      selling: false,
    };
    positions.set(key, position);
    watchForConfirmation(key, position);
  });
}

/**
 * Waits for the buy to land by watching the token account itself. The notification carries
 * the account data, so the exact balance to sell comes with the trigger.
 */
function watchForConfirmation(key: string, position: OpenPosition) {
  position.subscription = solanaConnection.onAccountChange(
    position.tokenAccount,
    (account) => {
      const amount = amountFromAccountData(account.data as Buffer);
      if (amount === 0n || position.confirmedAt) return;
      position.confirmedAt = Date.now();
      logger.info(
        `buy confirmed ${key} amount ${amount} after ${position.confirmedAt - position.detectedAt}ms`,
      );
      position.timer = setTimeout(() => exitPosition(key, amount), HOLD_AFTER_CONFIRM_MS);
    },
    'processed' as Commitment,
  );

  // the buy may simply not have landed; do not leak the subscription
  setTimeout(() => {
    const p = positions.get(key);
    if (p && !p.confirmedAt) {
      logger.info(`${key}: buy never landed, dropping`);
      cleanup(key);
    }
  }, BUY_TIMEOUT_MS);
}

async function exitPosition(key: string, amount: bigint) {
  const position = positions.get(key);
  if (!position || position.selling || !pumpGlobal) return;
  position.selling = true;

  try {
    // Helius Sender takes staked connections and wants its own tip
    const signature = await sellPosition(solanaConnection, wallet, pumpGlobal, position, amount, {
      cuPrice: SELL_CU_PRICE,
      tipAccount: HELIUS_SENDER_TIP_ACCOUNTS[Math.floor(Math.random() * HELIUS_SENDER_TIP_ACCOUNTS.length)],
      tipLamports: SELL_TIP_LAMPORTS,
      sender: sendTransactionHeliusSender,
    });
    const held = position.confirmedAt ? Date.now() - position.confirmedAt : 0;
    logger.info(`sold ${key} amount ${amount} held ${held}ms https://solscan.io/tx/${signature}`);
    sendMessage(`Sold ${key}`);

    await new Promise((resolve) => setTimeout(resolve, 4000));
    const info = await solanaConnection.getAccountInfo(position.tokenAccount, 'confirmed');
    if (info && amountFromAccountData(info.data) > 0n) {
      logger.warn(`${key}: still holding, retrying sell`);
      position.selling = false;
      setTimeout(() => exitPosition(key, amountFromAccountData(info.data)), 1000);
      return;
    }
    cleanup(key);
    await reportBalance();
  } catch (e) {
    logger.warn(`${key}: sell failed: ${(e as Error).message}`);
    position.selling = false;
    if (!softExit) setTimeout(() => exitPosition(key, amount), 1200);
  }
}

function cleanup(key: string) {
  const position = positions.get(key);
  if (!position) return;
  if (position.timer) clearTimeout(position.timer);
  if (position.subscription !== undefined) {
    solanaConnection.removeAccountChangeListener(position.subscription).catch(() => {});
  }
  positions.delete(key);
}

async function reportBalance() {
  const balance = await solanaConnection.getBalance(wallet.publicKey, 'confirmed' as Commitment);
  const current = balance / LAMPORTS_PER_SOL;
  const diff = current - initialWalletBalance;
  logger.info(`${diff > 0 ? 'Trade won' : 'Trade loss'} diff ${diff.toFixed(4)} balance ${current.toFixed(4)}`);
  initialWalletBalance = current;
}

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
import { getAssociatedTokenAddressSync } from '@solana/spl-token';
import {
  PUMP_PROGRAM,
  TOKEN_PROGRAM,
  TOKEN_2022_PROGRAM,
  ASSOCIATED_TOKEN_PROGRAM,
  SEED_BONDING_CURVE,
} from './pumpFun/constants';
import { parseLegs, readReserves, valueMultiple, decideLegSize, readCreatorBytes } from './pumpFun/ladder';

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
  // ladder exit state
  original: bigint;            // token balance at confirmation
  remaining: bigint;           // tokens still held
  entryCostLamports: bigint;   // SOL spent on the buy (cost basis)
  legTimers: NodeJS.Timeout[]; // scheduled ladder legs, cleared on full exit
  done: boolean;               // position fully closed
  finalRetries: number;        // capped hammering of a final leg that won't land
};

const positions = new Map<string, OpenPosition>();
// mints we gave up selling (migrated / program rejects the sell). Reconciliation must skip
// these or it re-adds and re-hammers them every sweep. Cleared only by a restart + manual sell.
const stuck = new Set<string>();
let pumpGlobal: PumpGlobal | undefined;
// A blockhash is valid ~60-90 s, so fetching one per sell leg just adds a serial RPC to the
// critical path. Refresh one in the background and hand it to every leg; a ~2 s-old hash is
// fine and shaves a round trip off each sell so the leg lands nearer its target slot.
let cachedBlockhash: string | undefined;
async function refreshBlockhash() {
  try {
    cachedBlockhash = (await solanaConnection.getLatestBlockhash('confirmed')).blockhash;
  } catch {
    /* keep the previous; sellPosition falls back to a fresh fetch if this is undefined */
  }
}
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

/**
 * Laddered exit, matching E4EzXdwf's behaviour and the backtested/validated ladder.
 *
 * Sells the position in scheduled legs rather than one dump: 30% at slot +1, 20% at +6/+9,
 * 15% at +13/+18, and whatever remains at +24. The early unconditional legs are what protect
 * against a dev dump between slots (the live A/B showed the schedule, not a reactive stop,
 * saves the fast-crash tokens). Two price-conditional overrides at each leg: if the position
 * has fallen to <= STOP_X of cost, dump everything now; if it is >= MOON_X, trim only
 * MOON_FRAC and keep riding.
 *
 * SELL_LADDER=0 falls back to the old single dump at HOLD_AFTER_CONFIRM_MS - an instant
 * escape hatch if the ladder ever misbehaves live.
 */
const USE_LADDER = (process.env.SELL_LADDER ?? '1') === '1';
const SLOT_MS = Number(process.env.SELL_SLOT_MS ?? 400);
const LADDER_LEGS: Array<[number, number]> =
  parseLegs(process.env.SELL_LADDER_LEGS) ?? [
    [1, 0.30], [6, 0.20], [9, 0.20], [13, 0.15], [18, 0.15],
  ];
const LADDER_LAST_SLOT = Number(process.env.SELL_LAST_SLOT ?? 24);
const STOP_X = Number(process.env.SELL_STOP_X ?? 0.8);
const MOON_X = Number(process.env.SELL_MOON_X ?? 2.0);
const MOON_FRAC = Number(process.env.SELL_MOON_FRAC ?? 0.05);
const DUST = 1000n; // ignore sub-dust remainders when deciding a position is closed
const RECONCILE_MS = Number(process.env.RECONCILE_MS ?? 60_000);
// max_sol_cost = budget × (1 + slippage_bps). The actual spend ≈ budget (the haircut sizes
// the token amount), so the fill's maxSolCost overstates the cost basis by the slippage. Back
// it out or every value multiple reads low and the 0.8× stop fires early (~0.84× real).
const SLIPPAGE_BPS = Number(process.env.SNIPER_SLIPPAGE_BPS ?? 500);

/** current value multiple of the remaining position vs its cost basis; null if unreadable */
async function currentMultiple(p: OpenPosition): Promise<number | null> {
  try {
    const info = await solanaConnection.getAccountInfo(p.bondingCurve, 'processed');
    if (!info) return null;
    const r = readReserves(info.data as Buffer);
    if (!r) return null;
    return valueMultiple(r.vSol, r.vToken, p.remaining, p.original, p.entryCostLamports);
  } catch {
    return null;
  }
}

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

  // keep a warm blockhash for the sell path
  await refreshBlockhash();
  setInterval(() => {
    refreshBlockhash().catch(() => {});
  }, 2000);

  // backstop: force-out any bag we are not tracking, on boot and on a timer. This is what
  // makes "we always sell" true even when a fill was dropped or a buy landed late.
  reconcileOrphans().catch((e) => logger.warn(`initial reconcile failed: ${(e as Error).message}`));
  setInterval(() => {
    reconcileOrphans().catch((e) => logger.warn(`reconcile failed: ${(e as Error).message}`));
  }, RECONCILE_MS);
}

function subscribeToFills(port: string) {
  const client = new ShredstreamProxyClient(`localhost:${port}`, credentials.createInsecure());
  const stream = client.subscribeFills(new SubscribeFillsRequest());

  let reconnected = false;
  const reconnect = (why: string) => {
    if (reconnected) return; // 'error' and 'end' can both fire; reconnect once
    reconnected = true;
    logger.warn(`fills stream ${why}, resubscribing`);
    setTimeout(() => subscribeToFills(port), 1000);
  };
  stream.on('error', (err) => reconnect(`error: ${err.message}`));
  // a broadcast lag on the proxy ends the gRPC stream cleanly (no 'error'); without this the
  // seller would go permanently deaf while the sniper keeps buying.
  stream.on('end', () => reconnect('ended'));
  stream.on('close', () => reconnect('closed'));

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
      original: 0n,
      remaining: 0n,
      // cost basis = the priced budget, not max_sol_cost (which is budget × (1+slippage))
      entryCostLamports: (BigInt(fill.getMaxSolCost() || 0) * 10_000n) / BigInt(10_000 + SLIPPAGE_BPS),
      legTimers: [],
      done: false,
      finalRetries: 0,
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
      position.original = amount;
      position.remaining = amount;
      logger.info(
        `buy confirmed ${key} amount ${amount} after ${position.confirmedAt - position.detectedAt}ms`,
      );
      if (USE_LADDER) {
        scheduleLadder(key);
      } else {
        // escape hatch: single dump at the old fixed hold
        position.timer = setTimeout(() => runLeg(key, LADDER_LAST_SLOT, null), HOLD_AFTER_CONFIRM_MS);
      }
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

/** Schedules every ladder leg plus the force-out. Legs fire on wall-clock timers measured
 *  from confirmation; each is guarded so a fired-then-closed position is a no-op. */
function scheduleLadder(key: string) {
  const p = positions.get(key);
  if (!p) return;
  const legs: Array<[number, number | null]> = [
    ...LADDER_LEGS.filter(([slot]) => slot < LADDER_LAST_SLOT),
    [LADDER_LAST_SLOT, null], // force-out: sell whatever remains
  ];
  p.legTimers = legs.map(([slot, frac]) =>
    setTimeout(() => {
      runLeg(key, slot, frac).catch((e) =>
        logger.warn(`${key}: leg slot+${slot} errored: ${(e as Error).message}`),
      );
    }, Math.max(0, slot * SLOT_MS)),
  );
}

/** One ladder leg. `frac` is the fraction of the ORIGINAL position to sell, or null for the
 *  force-out (sell everything remaining). Price overrides: <=STOP_X dumps all now, >=MOON_X
 *  trims only MOON_FRAC. A failed reserve read never blocks the scheduled sell.
 *
 *  `p.selling` is the per-position lock: acquired SYNCHRONOUSLY here before any await, so two
 *  legs whose timers fire together can never both send a sell (which would double-spend). */
async function runLeg(key: string, slot: number, frac: number | null): Promise<void> {
  const p = positions.get(key);
  if (!p || p.done) return;
  if (p.selling || !pumpGlobal) {
    // another leg/retry holds the lock (or global not ready). Do NOT drop this leg - under a
    // slow RPC stretch that would collapse the ladder toward the worst force-out@last. Re-queue
    // shortly; the lock is always released in the finally below, so this is bounded.
    setTimeout(() => runLeg(key, slot, frac).catch(() => {}), 250);
    return;
  }
  p.selling = true; // lock, synchronously, before the first await
  try {
    // the balance read (correctness across retries) and the curve read (the price multiple)
    // are independent - do them in parallel so a leg does one RTT, not two, and lands nearer
    // its target slot. A failed read never blocks the scheduled sell.
    let remaining = p.remaining;
    const [balInfo, m] = await Promise.all([
      solanaConnection.getAccountInfo(p.tokenAccount, 'processed').catch(() => null),
      currentMultiple(p),
    ]);
    if (balInfo) remaining = amountFromAccountData(balInfo.data as Buffer);
    if (remaining <= DUST) {
      p.remaining = 0n;
      p.done = true;
      return; // finalize handled in the finally via done
    }
    p.remaining = remaining;

    const { sellTokens, isFinal } = decideLegSize(p.original, remaining, frac, m, {
      stopX: STOP_X,
      moonX: MOON_X,
      moonFrac: MOON_FRAC,
      dust: DUST,
    });
    if (sellTokens <= 0n) return;

    await sendSell(key, p, sellTokens, isFinal, slot);
  } finally {
    if (p) p.selling = false;
  }
  // release happened above; act on terminal state outside the lock
  const done = positions.get(key)?.done;
  if (done) await finalize(key);
}

/** Sends one sell (partial or final). Caller holds the `selling` lock, so exactly one send is
 *  in flight per position. minSolOutput 0 so it never reverts on price - on an exit we take the
 *  fill. Closes the account only on the final leg.
 *
 *  A single submit only. On failure we NEVER blindly re-send the same token amount: a submit
 *  that actually landed but whose HTTP response timed out would then sell twice. Instead a
 *  failed final leg schedules a fresh runLeg force-out, which re-reads the on-chain balance
 *  before deciding how much to sell - so a double-fill is impossible. A failed partial leg is
 *  simply left for the next scheduled leg (which also re-reads); the tokens roll forward. */
async function sendSell(
  key: string,
  p: OpenPosition,
  tokens: bigint,
  isFinal: boolean,
  slot: number,
): Promise<void> {
  try {
    const signature = await sellPosition(solanaConnection, wallet, pumpGlobal!, p, tokens, {
      cuPrice: SELL_CU_PRICE,
      tipAccount: HELIUS_SENDER_TIP_ACCOUNTS[Math.floor(Math.random() * HELIUS_SENDER_TIP_ACCOUNTS.length)],
      tipLamports: SELL_TIP_LAMPORTS,
      minSolOutput: 0n,
      close: isFinal,
      blockhash: cachedBlockhash, // warm hash; sellPosition fetches one only if undefined
      sender: sendTransactionHeliusSender,
    });
    p.remaining = p.remaining > tokens ? p.remaining - tokens : 0n;
    logger.info(
      `sold ${key} slot+${slot} tokens ${tokens} ${isFinal ? 'FINAL' : 'leg'} ` +
        `remaining ${p.remaining} https://solscan.io/tx/${signature}`,
    );
    if (isFinal) p.done = true;
  } catch (e) {
    logger.warn(`${key}: sell slot+${slot} failed: ${(e as Error).message}`);
    // final leg must eventually close: re-enter runLeg (re-reads balance, never double-sells).
    // Capped, so a migrated/unsellable mint does not spin forever (reconcile would re-add it
    // every 60 s otherwise). After the cap, alert and leave it for manual review.
    if (isFinal && !softExit) {
      p.finalRetries += 1;
      if (p.finalRetries <= 8) {
        setTimeout(() => runLeg(key, LADDER_LAST_SLOT, null).catch(() => {}), 1200);
      } else {
        logger.error(`${key}: final sell failed ${p.finalRetries}x, giving up - MANUAL REVIEW`);
        sendMessage(`STUCK ${key}: cannot sell after ${p.finalRetries} tries - manual review`);
        stuck.add(key); // reconcile skips this so it does not re-add and re-hammer forever
        cleanup(key);
      }
    }
    // partial leg: leave it; the next scheduled leg re-reads and covers the remainder.
  }
}

/** Confirms the account is empty, reports, and cleans up. */
async function finalize(key: string) {
  const p = positions.get(key);
  if (!p) return;
  try {
    await new Promise((resolve) => setTimeout(resolve, 4000));
    const info = await solanaConnection.getAccountInfo(p.tokenAccount, 'confirmed');
    if (info && amountFromAccountData(info.data) > DUST) {
      logger.warn(`${key}: still holding after final leg, dumping remainder`);
      p.done = false;
      p.remaining = amountFromAccountData(info.data);
      await runLeg(key, LADDER_LAST_SLOT, null); // re-acquire lock, force-out; re-finalizes on success
      return;
    }
  } catch {
    /* best-effort verification */
  }
  sendMessage(`Closed ${key}`);
  cleanup(key);
  await reportBalance();
}

function cleanup(key: string) {
  const position = positions.get(key);
  if (!position) return;
  if (position.timer) clearTimeout(position.timer);
  for (const t of position.legTimers) clearTimeout(t);
  if (position.subscription !== undefined) {
    solanaConnection.removeAccountChangeListener(position.subscription).catch(() => {});
  }
  positions.delete(key);
}

/**
 * Orphan reconciliation - the real backstop that makes "always sell" true. A bag can slip the
 * tracker any number of ways: the fill was dropped because the seller was down, a durable-nonce
 * buy landed after we gave up, an RPC blip, a crash. None of those are recoverable from the
 * fill stream. So on boot and every RECONCILE_MS, enumerate the wallet's own token accounts and
 * force-out anything held that we are not already tracking. Bags cannot be held forever.
 *
 * The seeded token account is not an ATA, but we do not need the seed to sell it: the sell only
 * needs the account address (which the enumeration gives) plus the mint's derivable curve
 * accounts and the creator, which is read from the bonding-curve account.
 */
async function reconcileOrphans(): Promise<void> {
  if (!pumpGlobal) return; // runLeg needs it; registering without it would strand the bag
  for (const program of [TOKEN_PROGRAM, TOKEN_2022_PROGRAM]) {
    try {
      const resp = await solanaConnection.getParsedTokenAccountsByOwner(wallet.publicKey, {
        programId: program,
      });
      for (const { pubkey, account } of resp.value) {
        const info = (account.data as any).parsed?.info;
        if (!info) continue;
        const amountRaw = BigInt(info.tokenAmount?.amount ?? '0');
        if (amountRaw <= DUST) continue;
        const mint = new PublicKey(info.mint);
        const key = mint.toBase58();
        if (positions.has(key) || stuck.has(key)) continue; // tracked, selling, or given up

        const bondingCurve = PublicKey.findProgramAddressSync(
          [SEED_BONDING_CURVE, mint.toBuffer()],
          PUMP_PROGRAM,
        )[0];
        const curveInfo = await solanaConnection.getAccountInfo(bondingCurve, 'confirmed');
        const creatorBytes = curveInfo ? readCreatorBytes(curveInfo.data) : null;
        if (!creatorBytes) {
          logger.warn(`orphan ${key}: creator unreadable, leaving for manual review`);
          sendMessage(`ORPHAN ${key} bal ${amountRaw} - cannot auto-sell (no creator)`);
          continue;
        }
        const creator = new PublicKey(creatorBytes);
        const associatedBondingCurve = getAssociatedTokenAddressSync(
          mint,
          bondingCurve,
          true,
          program,
          ASSOCIATED_TOKEN_PROGRAM,
        );
        const position: OpenPosition = {
          mint,
          tokenAccount: pubkey,
          seed: '',
          tokenProgram: program,
          bondingCurve,
          associatedBondingCurve,
          creator,
          slot: 0,
          detectedAt: Date.now(),
          confirmedAt: Date.now(),
          selling: false,
          original: amountRaw,
          remaining: amountRaw,
          entryCostLamports: 0n, // unknown -> force-out ignores the price multiple
          legTimers: [],
          done: false,
          finalRetries: 0,
        };
        positions.set(key, position);
        logger.warn(`RECONCILE orphan ${key} balance ${amountRaw} - forcing out`);
        sendMessage(`Reconciling orphan ${key}`);
        runLeg(key, LADDER_LAST_SLOT, null).catch((e) => {
          // if the force-out did not start, drop the tracking so the next sweep retries
          // rather than skipping a still-held bag as "already tracked".
          logger.warn(`reconcile ${key} force-out failed: ${(e as Error).message}`);
          const p = positions.get(key);
          if (p && !p.done && !p.selling) positions.delete(key);
        });
      }
    } catch (e) {
      logger.warn(`reconcile sweep (${program.toBase58().slice(0, 4)}) failed: ${(e as Error).message}`);
    }
  }
}

async function reportBalance() {
  const balance = await solanaConnection.getBalance(wallet.publicKey, 'confirmed' as Commitment);
  const current = balance / LAMPORTS_PER_SOL;
  const diff = current - initialWalletBalance;
  logger.info(`${diff > 0 ? 'Trade won' : 'Trade loss'} diff ${diff.toFixed(4)} balance ${current.toFixed(4)}`);
  initialWalletBalance = current;
}

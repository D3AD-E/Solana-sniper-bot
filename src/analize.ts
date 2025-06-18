import { AnchorProvider } from '@coral-xyz/anchor';
import { PumpFunSDK } from 'pumpdotfun-sdk';
import { getProvider } from './anchor';
import { solanaSlowConnection, wallet } from './solana';
import { LAMPORTS_PER_SOL, Logs, PublicKey, VersionedTransactionResponse } from '@solana/web3.js';
import { Context } from '@solana/web3.js';
import { PumpBoughtEvent, PumpFunTradeEvent } from './analysis/analysis.types';
import { getRedisClient, lRange, rPush, sAdd, sMembers, sRem } from './redis';
import { countOccurrences } from './utils/arrayUtils';
import { PumpFunEstimator } from './pumpFun/estimator';
import { ALL_REDIS_WALLETS_KEY, REDIS_BOUGHT_DIFFS, REDIS_WALLETS_KEY } from './redis/redis.consts';

let provider: AnchorProvider | undefined = undefined;
let sdk: PumpFunSDK | undefined = undefined;
const seenCreatedTokens = new Map<string, PumpFunTradeEvent>();
const seenSignatures: Set<string> = new Set();
const listeningSockets = new Map<string, number>();
const walletBalances = new Map<string, number>();
const actionsPerToken = new Map<string, number>();

export async function deleteAllNetwork(wallet: string) {
  const redis = await getRedisClient();
  const queue = [wallet];
  const visited = new Set<string>(); // guards against loops

  while (queue.length) {
    const id = queue.shift();
    if (!id) break;
    if (visited.has(id)) continue;
    visited.add(id);

    const kids = await redis.sMembers(`rev:${id}`);
    queue.push(...kids);

    const moms = await redis.sMembers(`fwd:${id}`);
    queue.push(...moms);
  }
  if (visited.size === 0) return;
  const tx = redis.multi();
  visited.forEach((k) => tx.del(k));
  await tx.exec();
  visited.forEach((k) => removeWalletFromMonitoring(k));
}

async function monitorWallet(wallet: string, isInitial: boolean, prevWallet: string) {
  const redis = await getRedisClient();

  if ((await redis.sIsMember(REDIS_WALLETS_KEY, wallet)) === 1) return;
  if (!isInitial) {
    const wasAdded = await sAdd(REDIS_WALLETS_KEY, wallet);
    if (wasAdded === 0) return;
    if (prevWallet !== '') {
      await redis.hSet(`obj:${wallet}`, { wallet });
      await redis.sAdd(`fwd:${prevWallet}`, wallet);
      await redis.sAdd(`rev:${wallet}`, prevWallet);
    }
  } else {
    const wasAdded = await sAdd(ALL_REDIS_WALLETS_KEY, wallet);
    if (wasAdded === 0) return; // already monitoring this wallet
    const redis = await getRedisClient();
    await redis.hSet(`obj:${wallet}`, { wallet });
  }
  await monitorWalletNocheck(wallet);
}

async function monitorWalletNocheck(wallet: string) {
  const pubkey = new PublicKey(wallet);

  const subId = solanaSlowConnection.onLogs(pubkey, (logs, ctx) => {
    processTransactionLogs(logs, ctx, wallet);
  });
  listeningSockets.set(wallet, subId);
  console.log(`🔍 Monitoring wallet: ${wallet}, subscription ID: ${subId}`);
}

async function loadWallets() {
  await cleanEmptyWallets();
  const allWallets = await sMembers(REDIS_WALLETS_KEY);
  console.log('Loaded wallets:', allWallets.length);
  for (const wallet of allWallets) {
    const seenBalance = walletBalances.get(wallet) || 0;
    if (seenBalance > 100 * LAMPORTS_PER_SOL) {
      continue;
    }
    console.log(`Loading wallet: ${wallet}, balance: ${seenBalance / LAMPORTS_PER_SOL} SOL`);
    await monitorWalletNocheck(wallet);
  }
}

async function cleanEmptyWallets() {
  const allWallets = await sMembers(REDIS_WALLETS_KEY);
  const filteredWallets = allWallets.filter((wallet) => !seenCreatedTokens.has(wallet));
  for (const wallet of filteredWallets) {
    const balance = await solanaSlowConnection.getBalance(new PublicKey(wallet));
    if (balance / LAMPORTS_PER_SOL < 0.01) {
      await removeWalletFromMonitoring(wallet);
    } else {
      walletBalances.set(wallet, balance);
    }
  }
}

async function processTransactionLogs(logs: Logs, context: Context, wallet: string) {
  try {
    if (
      logs.logs.length > 6 ||
      countOccurrences(logs.logs, 'Program 11111111111111111111111111111111 invoke [1]') < 1
    ) {
      return;
    }
    if (seenSignatures.has(logs.signature)) return;
    const seenBalance = walletBalances.get(wallet) || 0;
    if (seenBalance > 100 * LAMPORTS_PER_SOL) {
      //some exchange probably
      return;
    } else {
      const currentBalance = await solanaSlowConnection.getBalance(new PublicKey(wallet));
      walletBalances.set(wallet, currentBalance);
      if (currentBalance > 1000 * LAMPORTS_PER_SOL) {
        //some exchange probably
        return;
      }
    }
    const transaction = await solanaSlowConnection.getTransaction(logs.signature, {
      maxSupportedTransactionVersion: 0,
    });
    if (seenSignatures.has(logs.signature)) return;
    seenSignatures.add(logs.signature);
    if (!transaction) return;

    const transfers = parseTransactionForSOL(transaction);
    // console.log(transfers);
    for (const transfer of transfers) {
      if (transfer.to === wallet) {
        continue;
      }
      if (transfer.amount > 0.1) monitorWallet(transfer.to, false, wallet);
    }
  } catch (error) {
    console.error(error);
    // Ignore errors for transactions we can't parse
  }
}

function parseTransactionForSOL(transaction: VersionedTransactionResponse) {
  const transfers: any[] = [];

  if (!transaction.meta) return transfers;

  const { preBalances, postBalances } = transaction.meta;
  const accountKeys = transaction.transaction.message.getAccountKeys();

  for (let i = 0; i < accountKeys.length; i++) {
    const preBalance = preBalances[i];
    const postBalance = postBalances[i];
    const balanceChange = postBalance - preBalance;

    if (balanceChange < 0) {
      const amountSent = Math.abs(balanceChange) / 1e9;

      // Find receiver
      for (let j = 0; j < accountKeys.length; j++) {
        if (i !== j) {
          const receiverChange = postBalances[j] - preBalances[j];
          if (receiverChange > 0) {
            transfers.push({
              signature: transaction.transaction.signatures[0],
              from: accountKeys.get(i)?.toString(),
              to: accountKeys.get(j)?.toString(),
              amount: amountSent,
              slot: transaction.slot,
            });
          }
        }
      }
    }
  }

  return transfers;
}

export default async function analize(): Promise<void> {
  provider = getProvider(true);
  sdk = new PumpFunSDK(provider);
  await loadWallets();
  //run every 5 minutes
  setInterval(
    () => {
      cleanEmptyWallets().catch(console.error);
    },
    5 * 60 * 1000,
  );

  setupPumpMonitor();

  // backTestPumpMonitor();
}

async function removeWalletFromMonitoring(wallet: string) {
  const subId = listeningSockets.get(wallet);
  if (subId) {
    solanaSlowConnection.removeOnLogsListener(subId);
    listeningSockets.delete(wallet);
  }
  await sRem(REDIS_WALLETS_KEY, wallet);
}

export function atomicUpdateSeenCreatedTokens(mint: string, data: Partial<PumpFunTradeEvent>): PumpFunTradeEvent {
  const existingData = seenCreatedTokens.get(mint);
  if (existingData) {
    seenCreatedTokens.set(mint, { ...existingData, ...data });
  } else {
    seenCreatedTokens.set(mint, data as PumpFunTradeEvent);
  }
  const newData = seenCreatedTokens.get(mint);
  return newData!;
}
async function setupPumpMonitor() {
  const redis = await getRedisClient();

  let tradeEvent = sdk!.addEventListener('tradeEvent', async (event, _, signature) => {
    try {
      if (!seenCreatedTokens.has(event.mint.toString())) {
        return;
      }
      let tokenData = seenCreatedTokens.get(event.mint.toString());
      if (tokenData === undefined) {
        return;
      }
      actionsPerToken.set(event.mint.toString(), (actionsPerToken.get(event.mint.toString()) || 0) + 1);
      let tokenBuySellDiff = event.isBuy ? tokenData.accValue + event.solAmount : tokenData.accValue - event.solAmount;
      if (Date.now() - tokenData.createdAt > 500 && !tokenData.otherSnipersList.includes(event.user.toString())) {
        tokenData = atomicUpdateSeenCreatedTokens(event.mint.toString(), { accValue: tokenBuySellDiff });
      } else {
        if (event.user.toString() === wallet.publicKey.toString() && event.isBuy) {
          console.log('Watch bought', event.mint.toString(), 'By', tokenData!.creator);
          tokenData = seenCreatedTokens.get(event.mint.toString());
          if (tokenData === undefined) {
            return;
          }
          tokenData.watchBought = true; //mark as watched
          tokenData = atomicUpdateSeenCreatedTokens(event.mint.toString(), { watchBought: true });
        }

        tokenData!.otherSnipersList.push(event.user.toString()); //dupe here to fix
      }

      if (tokenData.shouldBuy && Date.now() - tokenData.createdAt > 3800) {
        //donnt account for sniper buys, dont wait for event to handle this
        const moneyDiff =
          Date.now() - tokenData.createdAt > 5000 ||
          (event.user.toString() !== tokenData.creator && tokenData.otherSnipersList.includes(event.user.toString()))
            ? Number(tokenData.accValue) / LAMPORTS_PER_SOL
            : Number(tokenBuySellDiff) / LAMPORTS_PER_SOL;
        console.log(
          '🚨🚨🚨 Should have bought',
          event.mint.toString(),
          'By',
          tokenData.creator,
          moneyDiff,
          event.timestamp,
        );
        //   console.log(tokenData.otherSnipersList);
        await rPush(
          REDIS_BOUGHT_DIFFS,
          JSON.stringify({
            diff: moneyDiff,
            mint: event.mint.toString(),
            creator: tokenData.creator,
            createdAt: tokenData.createdAt,
            initialBuyAmount: Number(tokenData.initialBuyAmount),
          }),
        );
        tokenData = atomicUpdateSeenCreatedTokens(event.mint.toString(), {
          shouldBuy: false,
          accValue: tokenBuySellDiff,
        });
      }

      if (event.user.toString() === tokenData.creator) {
        if (event.isBuy) {
          //Initial buy
          tokenData = atomicUpdateSeenCreatedTokens(event.mint.toString(), {
            initialBuyAt: Date.now(),
            initialBuyAmount: event.solAmount,
          });
          return;
        }
        const timeSinceInitialBuy = Date.now() - (tokenData.initialBuyAt || 0);
        if (!event.isBuy && timeSinceInitialBuy < 4000) {
          console.log(tokenData.creator, event.mint.toString());
          await removeWalletFromMonitoring(tokenData.creator);
          if (tokenData.watchBought) await deleteAllNetwork(tokenData.creator);
        }
        if (
          tokenData.accValue > 1n * BigInt(LAMPORTS_PER_SOL / 4) &&
          actionsPerToken.get(event.mint.toString())! >= 4
        ) {
          if (timeSinceInitialBuy < 4000) {
            if (tokenData.shouldBuy) {
              if (tokenData.watchBought)
                console.log(
                  '🚨🚨🚨 Should have bought owner sold🚨',
                  event.mint.toString(),
                  'By',
                  tokenData.creator,
                  Number(tokenBuySellDiff) / LAMPORTS_PER_SOL,
                  event.timestamp,
                );
              await rPush(
                REDIS_BOUGHT_DIFFS,
                JSON.stringify({
                  diff: Number(tokenBuySellDiff) / LAMPORTS_PER_SOL,
                  mint: event.mint.toString(),
                  creator: tokenData.creator,
                  createdAt: tokenData.createdAt,
                  initialBuyAmount: Number(tokenData.initialBuyAmount),
                }),
              );
              tokenData = atomicUpdateSeenCreatedTokens(event.mint.toString(), {
                shouldBuy: false,
                accValue: tokenBuySellDiff,
              });
            }
            if (tokenData) await removeWalletFromMonitoring(tokenData.creator);
          } else {
            if (tokenData.watchBought)
              console.log(
                '💸 Creator sold with profit',
                event.mint.toString(),
                'By',
                event.user.toString(),
                Number(tokenData.accValue) / LAMPORTS_PER_SOL,
                'Time since initial buy:',
                timeSinceInitialBuy,
                'ms',
                new Date().toISOString(),
                'Bal diff',
                Number(event.solAmount - tokenData.initialBuyAmount!) / LAMPORTS_PER_SOL,
              );
            monitorWallet(event.user.toString(), false, '');
          }
        } else {
          if (tokenData.watchBought)
            console.log(
              '🟨 Creator sold with loss',
              event.mint.toString(),
              'By',
              event.user.toString(),
              Number(tokenData.accValue) / LAMPORTS_PER_SOL,
              'Bal diff',
              Number(event.solAmount - tokenData.initialBuyAmount!) / LAMPORTS_PER_SOL,
            );
          await removeWalletFromMonitoring(tokenData.creator);
          // await deleteAllNetwork(tokenData.creator);
        }
        seenCreatedTokens.delete(event.mint.toString());
        return;
      }
    } catch (e) {
      console.error(e);
    }
  });
  let createEvent = sdk!.addEventListener('createEvent', async (event, _, signature) => {
    actionsPerToken.set(event.mint.toString(), 0);
    seenCreatedTokens.set(event.mint.toString(), {
      creator: event.user.toString(),
      accValue: 0n,
      initialBuyAt: undefined,
      createdAt: Date.now(),
      otherSnipersList: [],
    });
    if ((await redis.sIsMember(REDIS_WALLETS_KEY, event.user.toString())) === 1) {
      atomicUpdateSeenCreatedTokens(event.mint.toString(), {
        shouldBuy: true,
      });
      await sAdd('seen_creator_addresses', event.user.toString());
    }
  });
}

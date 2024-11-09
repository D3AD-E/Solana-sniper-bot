import { TokenAmount, Token, TokenAccount, TOKEN_PROGRAM_ID } from '@raydium-io/raydium-sdk';
import { AccountLayout } from '@solana/spl-token';
import {
  Commitment,
  ComputeBudgetProgram,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import { u64 } from '@solana/buffer-layout-utils';
import { getTokenAccounts } from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import WebSocket from 'ws';
import { sendMessage } from './telegramBot';
import { Block } from './listener.types';
import { WorkerPool } from './workers/pool';
import { envVarToBoolean } from './utils/envUtils';
import { GlobalAccount, PumpFunSDK } from 'pumpdotfun-sdk';
import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import { buyPump, sellPump } from './pumpFun';
import Client, { SubscribeRequest } from '@triton-one/yellowstone-grpc';
import { bs58 } from '@coral-xyz/anchor/dist/cjs/utils/bytes';
import { JitoClient } from './jito/searcher';
import { struct, u32, u8 } from '@solana/buffer-layout';
import eventEmitter from './eventEmitter';
import { USER_STOP_EVENT } from './eventEmitter/eventEmitter.consts';
import { readFile, writeFile } from 'fs/promises';
import { BLACKLIST_FILE_NAME, LEADERS_FILE_NAME } from './constants';
import { SlotList } from 'jito-ts/dist/gen/block-engine/searcher';
import { readLeaders, refreshLeaders } from './jito/leaders';
let existingTokenAccounts: TokenAccount[] = [];

const quoteToken = Token.WSOL;
let swapAmount: TokenAmount;

let currentSlot = 0;
let ws: WebSocket | undefined = undefined;
let lastBlocks: Block[] = [];
let workerPool: WorkerPool | undefined = undefined;
let softExit = false;
let secondWallet: PublicKey | undefined = undefined;
let latestJitoTip: bigint | undefined = undefined;
let wasWonSeenTimeout = 0;

const getProvider = () => {
  const walletAnchor = new Wallet(wallet);
  const provider = new AnchorProvider(solanaConnection, walletAnchor, {
    commitment: process.env.COMMITMENT as Commitment,
  });
  return provider;
};

type BuyEvent = {
  timestamp: number;
};

type CurveMint = {
  mint: string;
  curve: PublicKey;
  otherPersonBuyAmount: bigint;
  otherPersonAddress: string;
};
type TipsData = {
  time: string; // ISO timestamp as a string
  landed_tips_25th_percentile: number;
  landed_tips_50th_percentile: number;
  landed_tips_75th_percentile: number;
  landed_tips_95th_percentile: number;
  landed_tips_99th_percentile: number;
  ema_landed_tips_50th_percentile: number;
};

let oldCurves: CurveMint[] = [];

let sdk: PumpFunSDK | undefined = undefined;
let gotTokenData = false;
let mintAccount = '';
let globalAccount: GlobalAccount | undefined = undefined;
let provider: AnchorProvider | undefined = undefined;
let shouldWeBuy = false;
let tokensSeen = 0;
let initialWalletBalance = 0;
let tokenBuySellDiff = 0n;
let buyEvents: BuyEvent[] = [];
let currentTips: TipsData | undefined = undefined;
let buyValues: string[] = [];
const pumpWallet = '12BRrNxzJYMx7cRhuBdhA71AchuxWRcvGydNnDoZpump';
const pumpWalletProfit = '4woK7yYzNqXSopwvU3J7un26w8XhowRhnWBbETFLfcaK';
let blackList: string[] = [];
let balance = 0;

eventEmitter.on(USER_STOP_EVENT, (data) => {
  softExit = true;
  console.log(softExit);
});

function calculateTokenAverage() {
  logger.info(`${tokensSeen} tokens`);
  sendMessage(`${tokensSeen} tokens`);
  tokensSeen = 0;
}

async function getWalletBalance() {
  balance = await solanaConnection.getBalance(wallet.publicKey);
}

function getAmountWeBuyBasedOnWalletFunds(currentBalance: number) {
  const totalAmountBN = BigInt(currentBalance);
  if (totalAmountBN > BigInt(1 * LAMPORTS_PER_SOL)) {
    const amountToBuy = totalAmountBN - totalAmountBN / 3n;
    return amountToBuy > BigInt(1 * LAMPORTS_PER_SOL) ? BigInt(1 * LAMPORTS_PER_SOL) : amountToBuy;
  }
  if (totalAmountBN < BigInt(1 * LAMPORTS_PER_SOL) && totalAmountBN > BigInt(0.5 * LAMPORTS_PER_SOL))
    return BigInt(0.3 * LAMPORTS_PER_SOL);
  const baseThreshold = BigInt(0.04 * LAMPORTS_PER_SOL);
  return totalAmountBN - baseThreshold;
}

function getAmountWeBuyBasedOnOther(otherPersonBuy: bigint, weWant: bigint) {
  if (otherPersonBuy >= 3_000_000_000n) return 0n;
  if (otherPersonBuy < 1_000_000_000n) return 0n;
  if (otherPersonBuy > 985_000_000n && otherPersonBuy < 988_000_000n) return 0n;
  return weWant!;
  // const initialStep = 500_000_000n;
  // if (otherPersonBuy <= initialStep) return buyAmountSol!;

  // let discountThreshold = initialStep;
  // let discountStep = 0n;
  // while (otherPersonBuy > discountThreshold) {
  //   discountThreshold += initialStep;
  //   discountStep = discountStep + 10n;
  //   if (discountStep === 90n) {
  //     return buyAmountSol! - (buyAmountSol! * discountStep) / 100n;
  //   }
  // }
  // console.log(discountStep, otherPersonBuy, initialStep, discountThreshold);
  // return buyAmountSol! - (buyAmountSol! * discountStep) / 100n;
}

function findCommonElement(array1: string[], array2: string[]) {
  for (let i = 0; i < array1.length; i++) {
    for (let j = 0; j < array2.length; j++) {
      if (array1[i].toLowerCase() === array2[j].toLowerCase()) {
        return true;
      }
    }
  }

  return false;
}

async function isAccNew(address: PublicKey) {
  try {
    // Step 1: Fetch the first transaction for the wallet
    const transactionSignatures = await solanaConnection.getSignaturesForAddress(address, {}, 'finalized');
    console.log(transactionSignatures);
    if (transactionSignatures.length === 0) {
      console.log('No transactions found for this wallet');
      return false; // No transactions found, so no way to check if it's from Binance
    }
    // Calculate the time difference in seconds
    const currentTime = Math.floor(Date.now() / 1000); // current time in seconds
    const elapsedTimeInSeconds = currentTime - transactionSignatures[transactionSignatures.length - 1].blockTime!;

    // Log and return elapsed time in various formats
    console.log(`Time since account creation: 
    ${elapsedTimeInSeconds} seconds 
    ${address.toString()} minutes `);
    console.log(transactionSignatures[0].blockTime, transactionSignatures[transactionSignatures.length - 1].slot);
    logger.info(`Here ${address.toString()}`);
  } catch (error) {
    console.error('Error checking transaction:', error);
    return false;
  }
}

function getOtherBuyValue(data: any) {
  try {
    const amountBuffer = data.slice(4);
    const reversedAmountBuffer = Buffer.from(amountBuffer).reverse();

    const buyValue = new BN(reversedAmountBuffer); // Use the relevant slice for the value
    return buyValue;
  } catch (e) {
    console.log(e);
  }
  return 0n;
}

async function subscribeToSnipeWonTransferUpdates() {
  const client = new Client('http://localhost:10000', 'args.xToken', {
    'grpc.max_receive_message_length': 64 * 1024 * 1024, // 64MiB
  });

  // Subscribe for events
  const stream = await client.subscribe();

  // Create `error` / `end` handler
  const streamClosed = new Promise<void>((resolve, reject) => {
    stream.on('error', (error) => {
      reject(error);
      stream.end();
    });
    stream.on('end', () => {
      resolve();
    });
    stream.on('close', () => {
      resolve();
    });
  });

  // Handle updates
  stream.on('data', async (data) => {
    if (data.transaction !== undefined) {
      console.log(data);
      wasWonSeenTimeout = new Date().getTime() + 20 * 60 * 1000;
      sendMessage('Started buy');
    }
  });
  // Create subscribe request based on provided arguments.
  const request: SubscribeRequest = {
    slots: {},
    accounts: {},
    transactions: {
      serum: {
        vote: false,
        accountInclude: [],
        accountExclude: [],
        accountRequired: [pumpWallet, pumpWalletProfit],
      },
    },
    blocks: {},
    blocksMeta: {},
    accountsDataSlice: [],
    transactionsStatus: {},
    entry: {},
  };
  // Send subscribe request
  await new Promise<void>((resolve, reject) => {
    stream.write(request, (err: any) => {
      if (err === null || err === undefined) {
        resolve();
      } else {
        reject(err);
      }
    });
  }).catch((reason) => {
    console.error(reason);
    throw reason;
  });

  await streamClosed;
}

async function subscribeToSnipeUpdates() {
  const client = new Client('http://localhost:10000', 'args.xToken', {
    'grpc.max_receive_message_length': 64 * 1024 * 1024, // 64MiB
  });

  // Subscribe for events
  const stream = await client.subscribe();

  // Create `error` / `end` handler
  const streamClosed = new Promise<void>((resolve, reject) => {
    stream.on('error', (error) => {
      reject(error);
      stream.end();
    });
    stream.on('end', () => {
      resolve();
    });
    stream.on('close', () => {
      resolve();
    });
  });

  // Handle updates
  stream.on('data', async (data) => {
    // if (isProcessing) return;
    if (softExit) return;
    const ins = data.transaction?.transaction?.meta?.innerInstructions;
    if (!ins) return;
    const signatureString = bs58.encode(data.transaction.transaction.signature);
    // logger.info('Snipe signature');
    // console.log(signatureString);
    if (ins.length !== 2) return;
    const instweIntrested = ins[1].instructions;
    try {
      for (const t of instweIntrested) {
        const dataBuffer = Buffer.from(t.data, 'base64');
        const opcode = dataBuffer.readUInt8(0); // First byte (should be 0x02 for transfer)
        if (opcode === 2) {
          const metaInstruction = data.transaction.transaction.transaction.message.instructions;
          const jitoTransfer = metaInstruction[metaInstruction.length - 1];
          const jitoBuffer = Buffer.from(jitoTransfer.data, 'base64');
          const jitoTip = getOtherBuyValue(jitoBuffer);
          const pumpBuy = getOtherBuyValue(dataBuffer);

          if (pumpBuy >= 600_000_000n) {
            buyEvents.push({ timestamp: new Date().getTime() });
            latestJitoTip = BigInt(jitoTip);
          }
          const now = Date.now();
          const filteredEvents = buyEvents.filter((event) => now - event.timestamp <= 120000);
          shouldWeBuy = filteredEvents.length >= 4;
          break;
        }
      }
    } catch (e) {
      console.error(e);
    }
  });
  // Create subscribe request based on provided arguments.
  const request: SubscribeRequest = {
    slots: {},
    accounts: {},
    transactions: {
      serum: {
        vote: false,
        accountInclude: [],
        accountExclude: [],
        accountRequired: [pumpWallet],
      },
    },
    blocks: {},
    blocksMeta: {},
    accountsDataSlice: [],
    transactionsStatus: {},
    entry: {},
  };
  // Send subscribe request
  await new Promise<void>((resolve, reject) => {
    stream.write(request, (err: any) => {
      if (err === null || err === undefined) {
        resolve();
      } else {
        reject(err);
      }
    });
  }).catch((reason) => {
    console.error(reason);
    throw reason;
  });

  await streamClosed;
}

async function subscribeToSlotUpdates() {
  const client = new Client('http://localhost:10000', 'args.xToken', {
    'grpc.max_receive_message_length': 64 * 1024 * 1024, // 64MiB
  });

  // Subscribe for events
  const stream = await client.subscribe();

  // Create `error` / `end` handler
  const streamClosed = new Promise<void>((resolve, reject) => {
    stream.on('error', (error) => {
      reject(error);
      stream.end();
    });
    stream.on('end', () => {
      resolve();
    });
    stream.on('close', () => {
      resolve();
    });
  });

  // Handle updates
  stream.on('data', async (data) => {
    // if (isProcessing) return;
    // if (softExit || new Date().getTime() > wasWonSeenTimeout) return;
    // if (!shouldWeBuy) return;
    const ins = data.transaction?.transaction?.meta?.innerInstructions;
    if (!ins) return;
    const signatureString = bs58.encode(data.transaction.transaction.signature);
    const instructionWithCurve = ins.find((x: any) => x.index === 5) ?? ins.find((x: any) => x.index === 4);
    if (!instructionWithCurve) return;
    if (
      !data.transaction?.transaction?.meta?.innerInstructions ||
      !data.transaction?.transaction?.meta?.innerInstructions[2]
    )
      return;
    logger.info('Signature');
    logger.info(signatureString);
    tokensSeen++;
    let tr2 = data.transaction?.transaction?.meta?.innerInstructions[2].instructions;
    let otherpersonBuyValue = 0n;
    for (const t of tr2) {
      const dataBuffer = Buffer.from(t.data, 'base64');
      const opcode = dataBuffer.readUInt8(0); // First byte (should be 0x02 for transfer)
      if (opcode === 2) {
        otherpersonBuyValue = getOtherBuyValue(dataBuffer);
        break;
      }
    }
    const pkKeys: PublicKey[] = data.transaction?.transaction?.transaction?.message?.accountKeys.map(
      (x: any) => new PublicKey(x),
    );
    const pkKeysStr = pkKeys.map((x) => x.toString().toLowerCase());
    // if (findCommonElement(pkKeysStr, blackList)) {
    //   logger.warn('Blacklisted');
    //   return;
    // }

    const mintAddress = ins[0].instructions[0].accounts[1];
    const mint = pkKeys[mintAddress];
    console.log('mint');
    if (mint === undefined) return;
    console.log(mint.toString());
    mintAccount = mint.toString();
    const curveAddress = instructionWithCurve.instructions[0].accounts[0];
    const curve = pkKeys[curveAddress];
    console.log('curve');
    console.log(curve);
    oldCurves.push({
      curve: curve,
      mint: mint.toString(),
      otherPersonBuyAmount: otherpersonBuyValue,
      otherPersonAddress: pkKeysStr[0],
    });
    await isAccNew(new PublicKey('CagF26EiddAmrnLhC5narPFB3FjVU53XjA1vVcK9JvnB'));
    const buySol = getAmountWeBuyBasedOnWalletFunds(balance);
    let weBuySol = getAmountWeBuyBasedOnOther(otherpersonBuyValue, buySol!);
    if (weBuySol === 0n) return;
    // await buyPump(
    //   wallet,
    //   mint,
    //   weBuySol!,
    //   calculateBuy(otherpersonBuyValue, weBuySol)!,
    //   globalAccount!,
    //   provider!,
    //   curve,
    //   lastBlocks[lastBlocks.length - 1],
    //   latestJitoTip!,
    // );
    logger.info('Sent buy');
  });
  // Create subscribe request based on provided arguments.
  const request: SubscribeRequest = {
    slots: {},
    accounts: {},
    transactions: {
      serum: {
        vote: false,
        accountInclude: [],
        accountExclude: [],
        accountRequired: [
          '6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P',
          'metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s',
          'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA',
          '11111111111111111111111111111111',
        ],
      },
    },
    blocks: {},
    blocksMeta: {},
    accountsDataSlice: [],
    transactionsStatus: {},
    entry: {},
  };
  // Send subscribe request
  await new Promise<void>((resolve, reject) => {
    stream.write(request, (err: any) => {
      if (err === null || err === undefined) {
        resolve();
      } else {
        reject(err);
      }
    });
  }).catch((reason) => {
    console.error(reason);
    throw reason;
  });

  await streamClosed;
}

function clearState() {
  console.log('Clearing state');
  mintAccount = '';
  gotTokenData = false;
}
function calculateBuy(otherPersonBuyAmount: bigint, weBuySol: bigint) {
  const otherPersonCorrected = BigInt(otherPersonBuyAmount);
  const otherBuy = globalAccount!.getInitialBuyPrice(otherPersonCorrected);
  const buyAmountTotal = globalAccount!.getInitialBuyPrice(otherPersonCorrected + weBuySol);
  return buyAmountTotal - otherBuy;
}

export default async function snipe(): Promise<void> {
  setInterval(storeRecentBlockhashes, 700);
  setInterval(getWalletBalance, 300);
  setInterval(calculateTokenAverage, 1000 * 60);
  sendMessage(`Started`);
  blackList = JSON.parse((await readFile(BLACKLIST_FILE_NAME)).toString()) as string[];
  console.log(blackList);
  await new Promise((resolve) => setTimeout(resolve, 5000));
  const client = await JitoClient.getInstance();
  logger.info('Starting');
  provider = getProvider();
  secondWallet = new PublicKey(process.env.SECOND_WALLET!);
  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );

  logger.info('Got token accounts');

  sdk = new PumpFunSDK(provider);
  globalAccount = await sdk.getGlobalAccount();
  // for (const tokenAccount of existingTokenAccounts) {
  //   if (
  //     tokenAccount.accountInfo.mint.toString().toLowerCase().endsWith('pump') &&
  //     tokenAccount.accountInfo.amount > 0
  //   ) {
  //     const sellResults = await sdk.sell(
  //       wallet,
  //       tokenAccount.accountInfo.mint,
  //       BigInt(tokenAccount.accountInfo.amount),
  //     );
  //     console.log(sellResults);
  //   }
  // }
  const buyAmountSol = BigInt(Number(process.env.SWAP_SOL_AMOUNT!) * LAMPORTS_PER_SOL);
  const balance = await solanaConnection.getBalance(wallet.publicKey);
  initialWalletBalance = balance / 1_000_000_000;
  console.log('Wallet balance (in SOL):', initialWalletBalance);
  // Call the subscription function
  subscribeToSlotUpdates();
  subscribeToSnipeUpdates();
  subscribeToSnipeWonTransferUpdates();
  // let tradeEvent = sdk!.addEventListener('tradeEvent', async (event, _, signature) => {
  //   if (event.user.toString().toLowerCase() === pumpWallet.toLowerCase()) {
  //     const token = oldCurves.find((x) => x.mint === event.mint.toString());
  //     if (token) {
  //       buyValues.push(token.otherPersonBuyAmount.toString());
  //       try {
  //         console.log('Writing');
  //         console.log(buyValues);
  //         await writeFile(LEADERS_FILE_NAME, JSON.stringify(buyValues));
  //       } catch (e) {}
  //     }
  //   }
  //   // logger.info(signature);
  //   // tokenBuySellDiff = event.isBuy ? tokenBuySellDiff + event.solAmount : tokenBuySellDiff - event.solAmount;
  //   // console.log('tradeEvent', event.isBuy ? 'Buy' : 'Sell', event.solAmount, 'Diff', tokenBuySellDiff);
  //   // tradesAmount++;
  // });
  await listenToChanges();
}

async function storeRecentBlockhashes() {
  try {
    const block = await solanaConnection.getLatestBlockhash('finalized');
    if (lastBlocks.length > 500) lastBlocks.splice(0, 100);
    lastBlocks.push(block);
  } catch (e) {
    logger.warn('Fetch blockhash failed');
    console.log(e);
  }
}

async function monitorSellLogic(currentMint: string, associatedCurve: PublicKey, otherPersonAddress: string) {
  console.log('Monitoring sell');
  sendMessage(`Monitoring sell for ${currentMint}`);

  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );
  let tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
  if (!tokenAccount || !tokenAccount.accountInfo) {
    console.log('curmint', currentMint);
    logger.warn('Unknown token');
    //another idea use the vents form swaps
    await new Promise((resolve) => setTimeout(resolve, 400));
    existingTokenAccounts = await getTokenAccounts(
      solanaConnection,
      wallet.publicKey,
      process.env.COMMITMENT as Commitment,
    );
    tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
    logger.warn('Unknown token2');
    console.log(tokenAccount);
    if (!tokenAccount || !tokenAccount.accountInfo) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      existingTokenAccounts = await getTokenAccounts(
        solanaConnection,
        wallet.publicKey,
        process.env.COMMITMENT as Commitment,
      );
      console.log(existingTokenAccounts.map((x) => x.accountInfo?.mint?.toString()));
      tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
      logger.warn('Unknown token3');
      console.log(tokenAccount);
      if (!tokenAccount || !tokenAccount.accountInfo) return true;
    }
  }

  //here we have tokenaccount
  //sell 1/3 instant

  const total = BigInt(tokenAccount.accountInfo.amount);
  console.log(total);
  if (total === 0n) return true;
  const firstPart = total / 2n;
  await new Promise((resolve) => setTimeout(resolve, 2200));
  await sellPump(
    wallet,
    tokenAccount.accountInfo.mint,
    total,
    globalAccount!,
    provider!,
    associatedCurve!,
    lastBlocks[lastBlocks.length - 1],
    latestJitoTip! / 10n,
  );

  await new Promise((resolve) => setTimeout(resolve, 5000));
  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );
  tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
  if (tokenAccount !== undefined && tokenAccount.accountInfo.amount !== undefined) {
    await sellPump(
      wallet,
      tokenAccount.accountInfo.mint,
      total,
      globalAccount!,
      provider!,
      associatedCurve!,
      lastBlocks[lastBlocks.length - 1],
      latestJitoTip! / 10n,
    );
  }
  logger.info('Sold all');
  await summaryPrint(otherPersonAddress);
  clearState();

  return false;
}

async function transferFunds() {
  const sendInstruction = SystemProgram.transfer({
    fromPubkey: wallet.publicKey,
    toPubkey: secondWallet!,
    lamports: 500_000_000n,
  });
  const messageV0 = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: lastBlocks[lastBlocks.length - 1].blockhash,
    instructions: [ComputeBudgetProgram.setComputeUnitLimit({ units: 72000 }), sendInstruction],
  }).compileToV0Message();

  const transaction = new VersionedTransaction(messageV0);
  transaction.sign([wallet]);
  const txid = await solanaConnection.sendTransaction(transaction, {
    skipPreflight: true,
  });
  console.log(txid);
}

async function summaryPrint(mint: string) {
  await new Promise((resolve) => setTimeout(resolve, 5000));
  const balance = await solanaConnection.getBalance(wallet.publicKey);
  const newWalletBalance = balance / 1_000_000_000;
  console.log('Wallet balance (in SOL):', newWalletBalance);
  if (newWalletBalance > 3.5) {
    sendMessage('Done');
    await transferFunds();
  }
  const diff = newWalletBalance - initialWalletBalance;
  console.log(diff > 0 ? 'Trade won' : 'Trade loss', 'Diff', diff);

  if (diff < -0.05 && diff > -0.3) {
    try {
      blackList.push(mint);
      await writeFile(BLACKLIST_FILE_NAME, JSON.stringify(blackList));
    } catch (e) {}
  }
  initialWalletBalance = newWalletBalance;
}

async function listenToChanges() {
  const walletSubscriptionId = solanaConnection.onProgramAccountChange(
    TOKEN_PROGRAM_ID,
    async (updatedAccountInfo) => {
      const accountData = AccountLayout.decode(updatedAccountInfo.accountInfo!.data);
      if (accountData.mint.toString() === quoteToken.mint.toString()) {
        const walletBalance = new TokenAmount(Token.WSOL, accountData.amount, true);
        logger.info('WSOL amount change ' + walletBalance.toFixed(4));
        sendMessage(`💸WSOL change ${walletBalance.toFixed(4)}`);
        return;
      }
      console.log(accountData.mint.toString() === mintAccount);

      if (accountData.mint.toString() === mintAccount) {
        logger.info(`Monitoring`);
        console.log(accountData.mint);
        // if (gotTokenData) return;
        // gotTokenData = true;
        const curve = oldCurves.find((x) => x.mint === accountData.mint.toString());
        if (curve) await monitorSellLogic(accountData.mint.toString(), curve.curve, curve.otherPersonAddress);
      } else {
        const curve = oldCurves.find((x) => x.mint === accountData.mint.toString());
        if (curve) await monitorSellLogic(accountData.mint.toString(), curve.curve, curve.otherPersonAddress);
      }
      // if (!workerPool!.doesTokenExist(accountData.mint.toString())) {
      //   logger.warn('Got unknown token in wallet');
      //   return;
      // }
      // processedTokens.push(accountData.mint.toString());
      // workerPool!.gotWalletToken(accountData.mint.toString(), accountData);
    },
    'processed' as Commitment,
    [
      {
        dataSize: 165,
      },
      {
        memcmp: {
          offset: 32,
          bytes: wallet.publicKey.toBase58(),
        },
      },
    ],
  );
}

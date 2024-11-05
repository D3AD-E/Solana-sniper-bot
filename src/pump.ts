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
let lastRequestDate = new Date().getTime();
let lastBlocks: Block[] = [];
let processedTokens: string[] = [];
let workerPool: WorkerPool | undefined = undefined;
let softExit = false;
let secondWallet: PublicKey | undefined = undefined;
let isProcessing = false;
let latestJitoTip: bigint | undefined = undefined;
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
let buyAmountSol: bigint | undefined = undefined;
let tokensSeen = 0;
let tradesAmount = 0;
let initialWalletBalance = 0;
let tokenBuySellDiff = 0n;
let buyEvents: BuyEvent[] = [];
let currentTips: TipsData | undefined = undefined;
let buyValues: string[] = [];
const pumpWallet = '12BRrNxzJYMx7cRhuBdhA71AchuxWRcvGydNnDoZpump';
let blackList: string[] = [];
let jitoData: {
  [key: string]: SlotList;
};

eventEmitter.on(USER_STOP_EVENT, (data) => {
  softExit = true;
  console.log(softExit);
});

function calculateTokenAverage() {
  logger.info(`${tokensSeen} tokens`);
  sendMessage(`${tokensSeen} tokens`);
  tokensSeen = 0;
}

function hasSlotInRange(jitoData: { [key: string]: SlotList }, target: number, tolerance: number): boolean {
  const lowerBound = target - tolerance;
  const upperBound = target + tolerance;
  console.log(target);
  console.log(currentSlot);
  for (const key in jitoData) {
    const slotList = jitoData[key].slots;
    for (const slot of slotList) {
      if (slot >= lowerBound && slot <= upperBound) {
        return true;
      }
    }
  }

  return false;
}

async function refreshCurrentSlot() {
  currentSlot = await solanaConnection.getSlot();
}

async function getJitoLeaders() {
  await refreshLeaders();
  jitoData = await readLeaders();
}

async function fetchTipsData(): Promise<void> {
  try {
    const response = await fetch('http://bundles-api-rest.jito.wtf/api/v1/bundles/tip_floor');

    if (!response.ok) {
      console.error('Failed to fetch data:', response.statusText);
      return;
    }

    const data: TipsData[] = await response.json();
    currentTips = data[0];
  } catch (error) {
    console.error('Error fetching tips data:', error);
  }
}

function getAmountWeBuyBasedOnOther(otherPersonBuy: bigint) {
  if (otherPersonBuy > 3_000_000_000n) return 0n;
  return buyAmountSol!;
  const initialStep = 500_000_000n;
  if (otherPersonBuy <= initialStep) return buyAmountSol!;

  let discountThreshold = initialStep;
  let discountStep = 0n;
  while (otherPersonBuy > discountThreshold) {
    discountThreshold += initialStep;
    discountStep = discountStep + 10n;
    if (discountStep === 90n) {
      return buyAmountSol! - (buyAmountSol! * discountStep) / 100n;
    }
  }
  console.log(discountStep, otherPersonBuy, initialStep, discountThreshold);
  return buyAmountSol! - (buyAmountSol! * discountStep) / 100n;
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
    logger.info('Snipe signature');
    console.log(signatureString);
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
            latestJitoTip = jitoTip;
          }
          const now = Date.now();
          const filteredEvents = buyEvents.filter((event) => now - event.timestamp <= 120000);
          shouldWeBuy = filteredEvents.length >= 2;
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
    if (softExit) return;
    if (!shouldWeBuy) return;
    const ins = data.transaction?.transaction?.meta?.innerInstructions;
    if (!ins) return;
    const signatureString = bs58.encode(data.transaction.transaction.signature);
    logger.info('Signature');
    console.log(signatureString);
    const instructionWithCurve = ins.find((x: any) => x.index === 5) ?? ins.find((x: any) => x.index === 4);
    if (!instructionWithCurve) return;
    if (
      !data.transaction?.transaction?.meta?.innerInstructions ||
      !data.transaction?.transaction?.meta?.innerInstructions[2]
    )
      return;
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
    if (findCommonElement(pkKeysStr, blackList)) {
      logger.warn('Blacklisted');
      return;
    }

    const mintAddress = ins[0].instructions[0].accounts[1];
    const mint = pkKeys[mintAddress];
    console.log('mint');
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
    let weBuySol = getAmountWeBuyBasedOnOther(otherpersonBuyValue);
    if (weBuySol === 0n) return;
    if (!hasSlotInRange(jitoData, currentSlot + 10, 3)) {
      logger.warn('No slot');
      return;
    }
    logger.info('Started listening');
    await buyPump(
      wallet,
      mint,
      weBuySol!,
      calculateBuy(otherpersonBuyValue, weBuySol)!,
      globalAccount!,
      provider!,
      curve,
      lastBlocks[lastBlocks.length - 1],
      latestJitoTip!,
    );
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
  isProcessing = false;
  gotTokenData = false;
  tradesAmount = 0;
}
function calculateBuy(otherPersonBuyAmount: bigint, weBuySol: bigint) {
  const otherPersonCorrected = BigInt(otherPersonBuyAmount);
  const otherBuy = globalAccount!.getInitialBuyPrice(otherPersonCorrected);
  const buyAmountTotal = globalAccount!.getInitialBuyPrice(otherPersonCorrected + weBuySol);
  return buyAmountTotal - otherBuy;
}

export default async function snipe(): Promise<void> {
  setInterval(storeRecentBlockhashes, 700);
  // setInterval(fetchTipsData, 500);
  setInterval(refreshCurrentSlot, 500);
  setInterval(getJitoLeaders, 1000 * 60 * 5);
  setInterval(calculateTokenAverage, 1000 * 60);
  sendMessage(`Started`);
  blackList = JSON.parse((await readFile(BLACKLIST_FILE_NAME)).toString()) as string[];
  await getJitoLeaders();
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
  buyAmountSol = BigInt(Number(process.env.SWAP_SOL_AMOUNT!) * LAMPORTS_PER_SOL);

  const balance = await solanaConnection.getBalance(wallet.publicKey);
  initialWalletBalance = balance / 1_000_000_000;
  console.log('Wallet balance (in SOL):', initialWalletBalance);
  // Call the subscription function
  subscribeToSlotUpdates();
  subscribeToSnipeUpdates();
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
  await new Promise((resolve) => setTimeout(resolve, 2800));
  await sellPump(
    wallet,
    tokenAccount.accountInfo.mint,
    total,
    globalAccount!,
    provider!,
    associatedCurve!,
    lastBlocks[lastBlocks.length - 1],
    latestJitoTip!,
  );
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
  if (newWalletBalance > 3) {
    sendMessage('Done');
    await transferFunds();
  }
  console.log(
    newWalletBalance - initialWalletBalance > 0 ? 'Trade won' : 'Trade loss',
    'Diff',
    newWalletBalance - initialWalletBalance,
  );

  if (newWalletBalance - initialWalletBalance < -0.02) {
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

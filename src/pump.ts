import { TokenAmount, Token, TokenAccount, TOKEN_PROGRAM_ID } from '@raydium-io/raydium-sdk';
import { AccountLayout } from '@solana/spl-token';
import {
  Commitment,
  ComputeBudgetProgram,
  LAMPORTS_PER_SOL,
  MessageCompiledInstruction,
  NonceAccount,
  PublicKey,
  SystemProgram,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import { getTokenAccounts } from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import WebSocket from 'ws';
import { sendMessage } from './telegramBot';
import { Block } from './listener.types';
import { GlobalAccount, PumpFunSDK } from 'pumpdotfun-sdk';
import { AnchorProvider, BN, Program } from '@coral-xyz/anchor';
import { buyPump, prebuildTx, sellPump } from './pumpFun';
import Client, { CommitmentLevel, SubscribeRequest } from '@triton-one/yellowstone-grpc';
import { bs58 } from '@coral-xyz/anchor/dist/cjs/utils/bytes';
import { JitoClient } from './jito/searcher';
import eventEmitter from './eventEmitter';
import { USER_STOP_EVENT } from './eventEmitter/eventEmitter.consts';
import { getProvider } from './anchor';
import { getRedisClient, sAdd } from './redis';
import { pingSlot, pingNextBlock, pingNode, pingAstra } from './keepAliveHttp/healthCheck';
import { getRandomBuyAmmount, DEFAULT_BUY_AMOUNT } from './slotTrade/constants';
import { IDL, PumpFun } from './pumpFun/IDL';
import { REDIS_SEEN_PK_SNIPE, REDIS_WALLETS_EXCHANGE_WHITELIST_KEY } from './redis/redis.consts';
import { credentials } from '@grpc/grpc-js';
import { ShredstreamProxyClient } from './generated/shredstream/shredstream_grpc_pb';
import { SubscribeEntriesRequest } from './generated/shredstream/shredstream_pb';
import { ParsedTx } from '../rust-native';
import { BuyTestData, CurveMint } from './listener.types';
import { TxType } from './pumpFun/pumpFun.types';
const { parseBuyTx } = require('../rust-native/index.node');

let existingTokenAccounts: TokenAccount[] = [];
let isMinimal: boolean = false;
const quoteToken = Token.WSOL;
let shredWs: WebSocket | undefined = undefined;
let lastBlocks: Block[] = [];
let softExit = false;
let secondWallet: PublicKey | undefined = undefined;
let program: Program<PumpFun> | undefined = undefined;

let buyDatas: Map<string, BuyTestData> = new Map<string, BuyTestData>();
let buyDatasShred: Map<string, BuyTestData> = new Map<string, BuyTestData>();
let oldCurves: CurveMint[] = [];

let sdk: PumpFunSDK | undefined = undefined;
let mintAccounts: string[] = [];
let globalAccount: GlobalAccount | undefined = undefined;
let provider: AnchorProvider | undefined = undefined;
let nonce = '';
let tokensSeen = 0;
let initialWalletBalance = 0;
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

async function getNonce() {
  const nonceAccountPubkey = new PublicKey(process.env.NONCE_PUBLIC_KEY!);

  const nonceAccountInfo = await solanaConnection.getAccountInfo(nonceAccountPubkey);

  if (!nonceAccountInfo) {
    throw new Error('Nonce account not found');
  }

  // Parse the nonce account to get the current nonce value
  const nonceAccount = NonceAccount.fromAccountData(nonceAccountInfo.data);
  nonce = nonceAccount.nonce;
}

function getAmountWeBuyBasedOnWalletFunds(currentBalance: number) {
  const totalAmountBN = BigInt(currentBalance);
  if (totalAmountBN < BigInt(0.4 * LAMPORTS_PER_SOL)) {
    logger.warn('Balance too low');
    return BigInt(0);
  }
  return getRandomBuyAmmount();
}

function getAmountWeBuyBasedOnOther(otherPersonBuy: bigint, weWant: bigint) {
  if (otherPersonBuy >= 2_900_000_000n) return 0n;
  return weWant!;
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

export async function walletExists(signer: string) {
  const redis = await getRedisClient();
  const isInWhitelist = (await redis.sIsMember(REDIS_WALLETS_EXCHANGE_WHITELIST_KEY, signer)) === 1;
  if (!isInWhitelist) return false;
  const walletData = await redis.hGetAll(`obj:${signer}`);

  if (walletData.dateOfExchangeTransfer !== undefined) {
    const walDate = Number(walletData.dateOfExchangeTransfer);
    if (Date.now() - walDate < 45 * 60 * 1000) {
      return false; // less than 5 minutes old from ecxhange
    }
  }
  return !(walletData === null || walletData.wallet === undefined);
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
      console.log(error);
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
    // if (gotTokenData) return;
    const ins = data.transaction?.transaction?.meta?.innerInstructions;
    if (!ins) return;
    const date = new Date().toISOString();
    let curveAddress = 0;
    let accKeyOfVaultIndex = 0;
    let otherpersonBuyValue = 0n;

    const instructionWithCurve = ins[ins.length - 1];
    if (!instructionWithCurve) return;
    try {
      if (instructionWithCurve.instructions[0].accounts.length > 3) {
        curveAddress = instructionWithCurve.instructions[1].accounts[0];
        accKeyOfVaultIndex = instructionWithCurve.instructions[2].accounts[1];
        const curveIndexTx = instructionWithCurve.instructions.find((x: any) => x.accounts.length == 3);
        const curveIndex = curveIndexTx.accounts[curveIndexTx.accounts.length - 1];
        let tr = instructionWithCurve.instructions.find(
          (x: any) => x.accounts.length == 2 && x.accounts[1] === curveIndex,
        );
        const dataBuffer = Buffer.from(tr.data, 'base64');
        const opcode = dataBuffer.readUInt8(0); // First byte (should be 0x02 for transfer)
        if (opcode === 2) {
          otherpersonBuyValue = getOtherBuyValue(dataBuffer);
        }
      } else {
        curveAddress = instructionWithCurve.instructions[0].accounts[0];
        accKeyOfVaultIndex = instructionWithCurve.instructions[1].accounts[1];
        const curveIndexTx = instructionWithCurve.instructions.find((x: any) => x.accounts.length == 3);
        const curveIndex = curveIndexTx.accounts[curveIndexTx.accounts.length - 1];

        let tr = instructionWithCurve.instructions.find(
          (x: any) => x.accounts.length == 2 && x.accounts[1] === curveIndex,
        );
        const dataBuffer = Buffer.from(tr.data, 'base64');
        const opcode = dataBuffer.readUInt8(0); // First byte (should be 0x02 for transfer)
        if (opcode === 2) {
          otherpersonBuyValue = getOtherBuyValue(dataBuffer);
        }
      }
    } catch (e) {
      console.error(e);
      return;
    }

    if (
      !data.transaction?.transaction?.meta?.innerInstructions ||
      !data.transaction?.transaction?.meta?.innerInstructions[2]
    )
      return;

    tokensSeen++;
    const pkKeys: PublicKey[] = data.transaction?.transaction?.transaction?.message?.accountKeys.map(
      (x: any) => new PublicKey(x),
    );
    const pkKeysStr = pkKeys.map((x) => x.toString());
    const ownerVault = pkKeys[accKeyOfVaultIndex];
    const curve = pkKeys[curveAddress];
    const signer = pkKeysStr[0];

    const mintAddress = ins[0].instructions[0].accounts[1];
    const mint = pkKeys[mintAddress];
    if (mint === undefined) return;
    const isWalletInWhiteList = await walletExists(signer);
    if (!isWalletInWhiteList) {
      return;
    }
    const buySol = getAmountWeBuyBasedOnWalletFunds(balance);
    let weBuySol = getAmountWeBuyBasedOnOther(otherpersonBuyValue, buySol!);
    if (weBuySol === 0n) return;
    const hasSeen = await sAdd(REDIS_SEEN_PK_SNIPE, mint.toString());
    if (hasSeen === 0) return;
    mintAccounts.push(mint.toString());
    oldCurves.push({
      curve: curve,
      mint: mint.toString(),
      otherPersonBuyAmount: otherpersonBuyValue,
      otherPersonAddress: pkKeysStr[0].toLowerCase(),
      ownerVault: ownerVault,
    });
    try {
      await buyPump(
        wallet,
        mint,
        weBuySol!,
        calculateBuy(otherpersonBuyValue, weBuySol)!,
        globalAccount!,
        curve,
        ownerVault,
        nonce,
        program!,
        isMinimal,
      );
      logger.info('Signature');
      const signatureString = bs58.encode(data.transaction.transaction.signature);
      logger.info(signatureString);
      logger.info('Sent buy yellow');
      console.log(date);
    } catch (e) {}
  });
  // Create subscribe request based on provided arguments.
  const request: SubscribeRequest = {
    slots: {},
    accounts: {},
    transactions: {
      serum: {
        vote: false,
        failed: false,
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
    commitment: CommitmentLevel.PROCESSED,
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

async function subscribeToOtherUpdates() {
  const client = new Client('http://localhost:10000', 'args.xToken', {
    'grpc.max_receive_message_length': 64 * 1024 * 1024, // 64MiB
  });

  // Subscribe for events
  const stream = await client.subscribe();

  // Create `error` / `end` handler
  const streamClosed = new Promise<void>((resolve, reject) => {
    stream.on('error', (error) => {
      console.log(error);
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
    const pkKeys: PublicKey[] = data?.transaction?.transaction?.transaction?.message?.accountKeys.map(
      (x: any) => new PublicKey(x),
    );
    if (!pkKeys) return;
    const pkKeysStr = pkKeys.map((x) => x.toString());
    const dateNow = Date.now();
    for (const key of pkKeysStr) {
      // console.log(key);
      const info = buyDatas.get(key);
      const infoShred = buyDatasShred.get(key);
      if (!info || info.wasSeen || !infoShred) continue;
      console.log(
        'Info other',
        info,
        dateNow - info!.boughtAt.getTime(),
        'ms (other vs geyser)',
        dateNow - infoShred!.boughtAt.getTime(),
        'ms (other vs shred)',
        info!.boughtAt.getTime() - infoShred!.boughtAt.getTime(),
        'ms (yellow vs shred)',
      );
      buyDatas.set(key, { ...info, wasSeen: true });
    }
  });
  // Create subscribe request based on provided arguments.
  const request: SubscribeRequest = {
    slots: {},
    accounts: {},
    transactions: {
      serum: {
        vote: false,
        failed: false,
        accountInclude: [],
        accountExclude: [],
        accountRequired: [
          'AcULspHXYRmoKkUhKN2WmFo4f8USUkmJtAE1LE2twJ2E',
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
    commitment: CommitmentLevel.PROCESSED,
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

async function subscribeToMyUpdates() {
  const client = new Client('http://localhost:10000', 'args.xToken', {
    'grpc.max_receive_message_length': 64 * 1024 * 1024, // 64MiB
  });

  // Subscribe for events
  const stream = await client.subscribe();

  // Create `error` / `end` handler
  const streamClosed = new Promise<void>((resolve, reject) => {
    stream.on('error', (error) => {
      console.log(error);
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
    const pkKeys: PublicKey[] = data?.transaction?.transaction?.transaction?.message?.accountKeys.map(
      (x: any) => new PublicKey(x),
    );
    if (!pkKeys) return;
    const pkKeysStr = pkKeys.map((x) => x.toString());
    const dateNow = Date.now();
    for (const key of pkKeysStr) {
      // console.log(key);
      const info = buyDatas.get(key);
      if (!info) continue;
      console.log('Info my', info, dateNow - info!.boughtAt.getTime(), 'ms');
      buyDatas.set(key, { ...info, wasSeen: true });
    }
  });
  // Create subscribe request based on provided arguments.
  const request: SubscribeRequest = {
    slots: {},
    accounts: {},
    transactions: {
      serum: {
        vote: false,
        failed: false,
        accountInclude: [],
        accountExclude: [],
        accountRequired: [
          wallet.publicKey.toString(),
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
    commitment: CommitmentLevel.PROCESSED,
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

function calculateBuy(otherPersonBuyAmount: bigint, weBuySol: bigint) {
  const otherPersonCorrected = BigInt(otherPersonBuyAmount);
  const otherBuy = globalAccount!.getInitialBuyPrice(otherPersonCorrected);
  const buyAmountTotal = globalAccount!.getInitialBuyPrice(otherPersonCorrected + weBuySol);
  return buyAmountTotal - otherBuy - 500n;
}

export default async function snipe(isMinimalRun: boolean = false): Promise<void> {
  if (process.env.COMMITMENT !== 'processed') throw new Error('Commitment invalid');
  isMinimal = isMinimalRun;
  setInterval(storeRecentBlockhashes, 700);
  setInterval(getWalletBalance, 300);
  setInterval(calculateTokenAverage, 1000 * 60);
  await pingSlot(); // immediate
  await pingNextBlock(); // immediate
  await pingNode(); // immediate
  await pingAstra(); // immediate
  setInterval(pingSlot, 50_000);
  setInterval(pingNextBlock, 50_000);
  setInterval(pingNode, 50_000);
  setInterval(pingAstra, 50_000);
  setInterval(getNonce, 1_000);

  sendMessage(`Started`);
  await new Promise((resolve) => setTimeout(resolve, 5000));
  const client = await JitoClient.getInstance();
  logger.info(`Starting in ${isMinimal ? 'minimal' : 'default'} mode`);
  provider = getProvider();
  program = new Program<PumpFun>(IDL as PumpFun, provider);
  secondWallet = new PublicKey(process.env.SECOND_WALLET!);

  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );
  logger.info('Got token accounts');

  sdk = new PumpFunSDK(provider);
  globalAccount = await sdk.getGlobalAccount();
  await getNonce();
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

  const balance = await solanaConnection.getBalance(wallet.publicKey);
  initialWalletBalance = balance / 1_000_000_000;
  console.log('Wallet balance (in SOL):', initialWalletBalance);

  await prebuildTx(globalAccount!, BigInt(DEFAULT_BUY_AMOUNT * LAMPORTS_PER_SOL), nonce, program, true);
  await prebuildTx(globalAccount!, BigInt(DEFAULT_BUY_AMOUNT * LAMPORTS_PER_SOL), nonce, program, false);
  setupShredStream('9999');

  if (!isMinimal) {
    setupShredStream('9998');
    subscribeToSlotUpdates();
    // subscribeToOtherUpdates();
    // subscribeToMyUpdates();
    await listenToChanges();
  }
}

function setupShredStreamWs() {
  shredWs = new WebSocket('ws://localhost:9001');
  shredWs.on('message', async function incoming(data) {
    const messageStr = data.toString();
    const jsonMessage = JSON.parse(messageStr);
    const ins = jsonMessage.message.instructions;
    if (!ins) return;
    const date = new Date().toISOString();
    let curveAddress = 0;
    let mintAddress = 0;
    let accKeyOfVaultIndex = 0;
    let otherpersonBuyValue = 0n;

    const instructionWithCurve = ins;
    if (!instructionWithCurve) return;
    try {
      const instructionWithCurveFilteredMaxAcc = instructionWithCurve.filter(
        (x: any) => x.accounts.length > 5,
      ) as any[];
      mintAddress = instructionWithCurveFilteredMaxAcc[0].accounts[0];
      curveAddress = instructionWithCurveFilteredMaxAcc[0].accounts[3];

      accKeyOfVaultIndex =
        instructionWithCurveFilteredMaxAcc[instructionWithCurveFilteredMaxAcc.length - 1].accounts[9];
      const curveIndexTx = instructionWithCurveFilteredMaxAcc[instructionWithCurveFilteredMaxAcc.length - 1];
      const dataBuffer = new Uint8Array(curveIndexTx.data);
      const hexArray = curveIndexTx.data.map((b: any) => `0x${b.toString(16).padStart(2, '0')}`);
      const amountBuffer = dataBuffer.slice(8, 16);
      const reversedAmountBuffer = Buffer.from(amountBuffer).reverse();
      const buyValue = new BN(reversedAmountBuffer);
      const x0 = new BN(globalAccount!.initialVirtualSolReserves);
      const y0 = new BN(globalAccount!.initialVirtualTokenReserves);

      // Do math using BN methods
      const numerator = x0.mul(buyValue);
      const denominator = y0.sub(buyValue);
      otherpersonBuyValue = numerator.div(denominator);
    } catch (e) {
      console.error(e);
      return;
    }
    const pkKeys: PublicKey[] = jsonMessage.message?.accountKeys.map((x: any) => new PublicKey(x));
    const pkKeysStr = pkKeys.map((x) => x.toString());
    const ownerVault = pkKeys[accKeyOfVaultIndex];
    const curve = pkKeys[curveAddress];
    const mint = pkKeys[mintAddress];
    const signer = pkKeysStr[0];
    tokensSeen++;
    const isWalletInWhiteList = await walletExists(signer);
    if (!isWalletInWhiteList) {
      return;
    }
    mintAccounts.push(mint.toString());
    oldCurves.push({
      curve: curve,
      mint: mint.toString(),
      otherPersonBuyAmount: otherpersonBuyValue,
      otherPersonAddress: signer.toLowerCase(),
      ownerVault: ownerVault,
    });
    const buySol = getAmountWeBuyBasedOnWalletFunds(balance);
    let weBuySol = getAmountWeBuyBasedOnOther(otherpersonBuyValue, buySol!);
    console.log(otherpersonBuyValue.toString(), weBuySol);
    if (weBuySol === 0n) return;
    const hasSeen = await sAdd(REDIS_SEEN_PK_SNIPE, mint.toString());
    if (hasSeen === 0) return;
    try {
      await buyPump(
        wallet,
        mint,
        weBuySol!,
        calculateBuy(otherpersonBuyValue, weBuySol)!,
        globalAccount!,
        curve,
        ownerVault,
        nonce,
        program!,
        isMinimal,
      );
      console.log(weBuySol);
      logger.info('Signature shred');
      console.log(mint.toString(), curve.toString(), otherpersonBuyValue.toString(), ownerVault.toString());
      logger.info('Sent buy shred default');
      console.log(date);
    } catch (e) {}
  });
}

function setupShredStream(port: string) {
  const shredClient = new ShredstreamProxyClient(`localhost:${port}`, credentials.createInsecure());

  const request = new SubscribeEntriesRequest();
  const stream = shredClient.subscribeEntries(request);

  stream.on('error', (err) => {
    console.error('Stream error:', err);
  });
  stream.on('data', async (entry) => {
    try {
      // const entriesList: string[] = entry.getEntriesList();
      // if (entriesList.length === 0) return;
      // const date = new Date();
      // const entryStr: string = entriesList[0];
      // const parsedTxPump: ParsedTx = parseBuyTx(
      //   entryStr,
      //   globalAccount!.initialVirtualSolReserves.toString(),
      //   globalAccount!.initialVirtualTokenReserves.toString(),
      // );
      // tokensSeen++;

      // const isWalletInWhiteList = await walletExists(parsedTxPump.signer);
      // if (!isWalletInWhiteList) {
      //   return;
      // }
      // const curve = new PublicKey(parsedTxPump.curve);
      // const mint = new PublicKey(parsedTxPump.mint);
      // const ownerVault = new PublicKey(parsedTxPump.ownerVault);
      // const otherpersonBuyValue = BigInt(parsedTxPump.otherValue);
      const entriesList: string[] = entry.getEntriesList();
      if (entriesList.length === 0) return;
      const buffer = Buffer.from(entriesList[0], 'base64');
      const tx = VersionedTransaction.deserialize(buffer);
      const ins = tx.message.compiledInstructions;
      if (!ins) return;
      const date = new Date().toISOString();
      let curveAddress = 0;
      let mintAddress = 0;
      let accKeyOfVaultIndex = 0;
      let otherpersonBuyValue = 0n;

      const instructionWithCurve = ins;
      if (!instructionWithCurve) return;
      try {
        const instructionWithCurveFilteredMaxAcc = instructionWithCurve.filter(
          (x: MessageCompiledInstruction) => x.accountKeyIndexes.length > 5,
        ) as MessageCompiledInstruction[];
        mintAddress = instructionWithCurveFilteredMaxAcc[0].accountKeyIndexes[0];
        curveAddress = instructionWithCurveFilteredMaxAcc[0].accountKeyIndexes[3];

        accKeyOfVaultIndex =
          instructionWithCurveFilteredMaxAcc[instructionWithCurveFilteredMaxAcc.length - 1].accountKeyIndexes[9];
        const curveIndexTx = instructionWithCurveFilteredMaxAcc[instructionWithCurveFilteredMaxAcc.length - 1];
        const amountBuffer = curveIndexTx.data.slice(8, 16);
        const reversedAmountBuffer = Buffer.from(amountBuffer).reverse();
        const buyValue = new BN(reversedAmountBuffer);
        const x0 = new BN(globalAccount!.initialVirtualSolReserves);
        const y0 = new BN(globalAccount!.initialVirtualTokenReserves);

        // Do math using BN methods
        const numerator = x0.mul(buyValue);
        const denominator = y0.sub(buyValue);
        otherpersonBuyValue = numerator.div(denominator);
      } catch (e) {
        console.error(e);
        return;
      }

      const pkKeys: PublicKey[] = tx.message.staticAccountKeys;
      const pkKeysStr = pkKeys.map((x) => x.toString());
      const ownerVault = pkKeys[accKeyOfVaultIndex];
      const curve = pkKeys[curveAddress];
      const mint = pkKeys[mintAddress];
      const signer = pkKeysStr[0];
      tokensSeen++;
      const isWalletInWhiteList = await walletExists(signer);
      if (!isWalletInWhiteList) {
        return;
      }
      mintAccounts.push(mint.toString());
      oldCurves.push({
        curve: curve,
        mint: mint.toString(),
        otherPersonBuyAmount: otherpersonBuyValue,
        otherPersonAddress: signer.toLowerCase(),
        ownerVault: ownerVault,
      });
      console.log(mint, otherpersonBuyValue.toString());
      buyDatasShred.set(mint.toString(), { mint: mint.toString(), boughtAt: new Date() });
      if (!isMinimal) {
        const hasSeen = await sAdd(REDIS_SEEN_PK_SNIPE, mint.toString());
        if (hasSeen === 0) return;
      }

      const buySol = getAmountWeBuyBasedOnWalletFunds(balance);
      let weBuySol = getAmountWeBuyBasedOnOther(otherpersonBuyValue, buySol!);
      if (weBuySol === 0n) {
        console.log('Too much');
        return;
      }
      await buyPump(
        wallet,
        mint,
        weBuySol!,
        calculateBuy(otherpersonBuyValue, weBuySol)!,
        globalAccount!,
        curve,
        ownerVault,
        nonce,
        program!,
        isMinimal,
      );
      // const buffer = Buffer.from(entryStr, 'base64');
      // const tx = VersionedTransaction.deserialize(buffer);
      console.log('Mint', mint.toString(), port);
      logger.info('Signature shred', mint.toString());
      const signatureBase58 = bs58.encode(tx.signatures[0]);
      console.log(`https://solscan.io/tx/${signatureBase58}`);
      logger.info('Sent buy shred direct', port);
      console.log(date);
    } catch (e) {
      console.error(e);
    }
  });
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

async function monitorSellLogic(
  currentMint: string,
  associatedCurve: PublicKey,
  otherPersonAddress: string,
  ownerVault: PublicKey,
) {
  console.log('Monitoring sell');
  sendMessage(`Monitoring sell for ${currentMint}`);

  existingTokenAccounts = await getTokenAccounts(solanaConnection, wallet.publicKey, 'processed');
  let tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
  if (!tokenAccount || !tokenAccount.accountInfo) {
    console.log('curmint', currentMint);
    logger.warn('Unknown token');
    //another idea use the vents form swaps
    await new Promise((resolve) => setTimeout(resolve, 400));
    existingTokenAccounts = await getTokenAccounts(solanaConnection, wallet.publicKey, 'processed');
    tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
    logger.warn('Unknown token2');
    console.log(tokenAccount);
    if (!tokenAccount || !tokenAccount.accountInfo) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      existingTokenAccounts = await getTokenAccounts(solanaConnection, wallet.publicKey, 'processed');
      console.log(existingTokenAccounts.map((x) => x.accountInfo?.mint?.toString()));
      tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
      logger.warn('Unknown token3');
      console.log(tokenAccount);
      if (!tokenAccount || !tokenAccount.accountInfo) return true;
    }
  }

  const total = BigInt(tokenAccount.accountInfo.amount);
  console.log(total);
  if (total === 0n) return true;
  const firstPart = total / 2n;
  await new Promise((resolve) => setTimeout(resolve, 2100));
  await sellPump(
    wallet,
    tokenAccount.accountInfo.mint,
    total,
    globalAccount!,
    provider!,
    associatedCurve!,
    lastBlocks[lastBlocks.length - 1],
    ownerVault,
    tokenAccount.pubkey,
  );

  await new Promise((resolve) => setTimeout(resolve, 5000));
  existingTokenAccounts = await getTokenAccounts(solanaConnection, wallet.publicKey, 'processed');
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
      ownerVault,
      tokenAccount.pubkey,
    );
  }
  logger.info('Sold all');
  await summaryPrint(otherPersonAddress);
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
  if (newWalletBalance > 5.5) {
    sendMessage('Done');
    await transferFunds();
  }
  const diff = newWalletBalance - initialWalletBalance;
  console.log(diff > 0 ? 'Trade won' : 'Trade loss', 'Diff', diff);

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
      console.log(mintAccounts.indexOf(accountData.mint.toString()) !== -1);
      logger.info(`Monitoring`);
      console.log(accountData.mint);
      await getNonce();
      const curve = oldCurves.find((x) => x.mint === accountData.mint.toString());
      if (curve)
        await monitorSellLogic(accountData.mint.toString(), curve.curve, curve.otherPersonAddress, curve.ownerVault);
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

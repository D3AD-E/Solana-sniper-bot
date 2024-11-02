import { TokenAmount, Token, TokenAccount, TOKEN_PROGRAM_ID } from '@raydium-io/raydium-sdk';
import { AccountLayout } from '@solana/spl-token';
import { Commitment, LAMPORTS_PER_SOL, PublicKey } from '@solana/web3.js';
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
let existingTokenAccounts: TokenAccount[] = [];

const quoteToken = Token.WSOL;
let swapAmount: TokenAmount;

let ws: WebSocket | undefined = undefined;

let lastBlocks: Block[] = [];
let processedTokens: string[] = [];
let workerPool: WorkerPool | undefined = undefined;

let isProcessing = false;
const getProvider = () => {
  const walletAnchor = new Wallet(wallet);
  const provider = new AnchorProvider(solanaConnection, walletAnchor, {
    commitment: process.env.COMMITMENT as Commitment,
  });
  return provider;
};

let sdk: PumpFunSDK | undefined = undefined;
let gotTokenData = false;
let mintAccount = '';
let globalAccount: GlobalAccount | undefined = undefined;
let provider: AnchorProvider | undefined = undefined;
let associatedCurve: PublicKey | undefined = undefined;
let isSelling = false;
let buyAmountSol: bigint | undefined = undefined;
let boughtTokens = 0;
let tradesAmount = 0;
let initialWalletBalance = 0;
let tokenBuySellDiff = 0n;
let otherPersonBuySol = 0n;
function isBuyDataOk(data: any) {
  try {
    const amountBuffer = data.slice(4);
    const reversedAmountBuffer = Buffer.from(amountBuffer).reverse();

    const buyValue = new BN(reversedAmountBuffer); // Use the relevant slice for the value
    console.log('Parsed BigNumber3:', buyValue.toString());
    otherPersonBuySol = buyValue;
    if (buyValue > 2500000000n) {
      logger.warn('Buy wrong');
      return false;
    } else return true;
  } catch (e) {
    console.log(e);
  }
  otherPersonBuySol = 0n;
  return false;
}
// Example of subscribing to slot updates
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
    if (isProcessing) return;
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
    let tr2 = data.transaction?.transaction?.meta?.innerInstructions[2].instructions;

    for (const t of tr2) {
      const dataBuffer = Buffer.from(t.data, 'base64');
      const opcode = dataBuffer.readUInt8(0); // First byte (should be 0x02 for transfer)
      if (opcode === 2) {
        if (isBuyDataOk(dataBuffer)) break;
        else return;
      }
    }
    if (isProcessing) return;
    isProcessing = true;
    const pkKeys: PublicKey[] = data.transaction?.transaction?.transaction?.message?.accountKeys.map(
      (x: any) => new PublicKey(x),
    );
    const mintAddress = ins[0].instructions[0].accounts[1];
    const mint = pkKeys[mintAddress];
    console.log('mint');
    console.log(mint.toString());
    mintAccount = mint.toString();
    const curveAddress = instructionWithCurve.instructions[0].accounts[0];
    const curve = pkKeys[curveAddress];
    console.log('curve');
    console.log(curve);
    associatedCurve = curve;

    logger.info('Started listening');
    await buyPump(
      wallet,
      mint,
      buyAmountSol!,
      calculateBuy(otherPersonBuySol)!,
      globalAccount!,
      provider!,
      curve,
      lastBlocks[lastBlocks.length - 1],
    );
    logger.info('Sent buy');
    const localBoughtTokens = boughtTokens;
    await new Promise((resolve) => setTimeout(resolve, 3000));
    //fix boughttokens
    console.log('Failbuy check', !gotTokenData, localBoughtTokens === boughtTokens);
    if (!gotTokenData && localBoughtTokens === boughtTokens) {
      logger.warn('Buy failed');
      clearState();
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
  associatedCurve = undefined;
  isProcessing = false;
  gotTokenData = false;
  tradesAmount = 0;
}
function calculateBuy(otherPersonBuyAmount: bigint) {
  logger.info('Calcbuy');
  const otherPersonCorrected = BigInt(otherPersonBuyAmount);
  const otherBuy = globalAccount!.getInitialBuyPrice(otherPersonCorrected);
  const buyAmountTotal = globalAccount!.getInitialBuyPrice(otherPersonCorrected + buyAmountSol!);
  return buyAmountTotal - otherBuy;
}

export default async function snipe(): Promise<void> {
  setInterval(storeRecentBlockhashes, 700);
  sendMessage(`Started`);

  await new Promise((resolve) => setTimeout(resolve, 5000));
  const client = await JitoClient.getInstance();
  logger.info('Starting');
  provider = getProvider();

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

  let tradeEvent = sdk!.addEventListener('tradeEvent', async (event, _, signature) => {
    if (event.mint.toString() === mintAccount) {
      if (event.user.toString() === wallet.publicKey.toString()) return;
      logger.info(signature);
      tokenBuySellDiff = event.isBuy ? tokenBuySellDiff + event.solAmount : tokenBuySellDiff - event.solAmount;
      console.log('tradeEvent', event.isBuy ? 'Buy' : 'Sell', event.solAmount, 'Diff', tokenBuySellDiff);
      tradesAmount++;
    }
  });
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

async function monitorSellLogic(currentMint: string) {
  console.log('Monitoring partial sell');
  sendMessage(`Monitoring partial sell for ${currentMint}`);

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
  await new Promise((resolve) => setTimeout(resolve, 3000));

  await sellPump(
    wallet,
    tokenAccount.accountInfo.mint,
    firstPart,
    globalAccount!,
    provider!,
    associatedCurve!,
    lastBlocks[lastBlocks.length - 1],
  );
  //sell 1/5 later
  const secondPart = total / 5n;
  await new Promise((resolve) => setTimeout(resolve, 15000));
  logger.info('Sell 2/4');
  await sellAll(currentMint);
  await summaryPrint();
  // clearState();

  return false;
  await sellPump(
    wallet,
    tokenAccount.accountInfo.mint,
    secondPart,
    globalAccount!,
    provider!,
    associatedCurve!,
    lastBlocks[lastBlocks.length - 1],
  );

  await new Promise((resolve) => setTimeout(resolve, 3900 * 60));
  logger.info('Sell 3/4');

  const thirdPart = total / 10n;
  console.log(tradesAmount);
  if (tradesAmount < 5) {
    logger.warn('Inactive pair');
    await sellAll(currentMint);
    await summaryPrint();
    clearState();

    return false;
  }
  await sellPump(
    wallet,
    tokenAccount.accountInfo.mint,
    thirdPart,
    globalAccount!,
    provider!,
    associatedCurve!,
    lastBlocks[lastBlocks.length - 1],
  );
  await new Promise((resolve) => setTimeout(resolve, 1800 * 60));
  logger.info('Sell 4/4');

  const forthPart = total / 20n;
  await sellPump(
    wallet,
    tokenAccount.accountInfo.mint,
    forthPart,
    globalAccount!,
    provider!,
    associatedCurve!,
    lastBlocks[lastBlocks.length - 1],
  );
  await new Promise((resolve) => setTimeout(resolve, 1800 * 60));
  //all
  await sellAll(currentMint);
  await summaryPrint();
  clearState();
  return false;
}

async function sellAll(currentMint: string) {
  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );
  const tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
  const newTotal = BigInt(tokenAccount.accountInfo.amount);
  console.log(newTotal);

  await sellPump(
    wallet,
    tokenAccount.accountInfo.mint,
    newTotal,
    globalAccount!,
    provider!,
    associatedCurve!,
    lastBlocks[lastBlocks.length - 1],
  );
  logger.info('Sold all');
}
async function summaryPrint() {
  await new Promise((resolve) => setTimeout(resolve, 5000));
  const balance = await solanaConnection.getBalance(wallet.publicKey);
  const newWalletBalance = balance / 1_000_000_000;
  console.log('Wallet balance (in SOL):', newWalletBalance);
  console.log(
    newWalletBalance - initialWalletBalance > 0 ? 'Trade won' : 'Trade loss',
    'Diff',
    newWalletBalance - initialWalletBalance,
  );
  sendMessage(
    `${newWalletBalance - initialWalletBalance > 0 ? 'Trade won' : 'Trade loss'} ${newWalletBalance - initialWalletBalance}`,
  );
  initialWalletBalance = newWalletBalance;
}

async function sellToken(currentMint: string) {
  console.log('Selling');
  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );
  let tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
  if (!tokenAccount || !tokenAccount.accountInfo) {
    console.log('curmint', currentMint);
    logger.warn('Unknown token');
    existingTokenAccounts = await getTokenAccounts(
      solanaConnection,
      wallet.publicKey,
      process.env.COMMITMENT as Commitment,
    );
    tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === currentMint)!;
    logger.warn('Unknown token2');
    console.log(tokenAccount);
    if (!tokenAccount || !tokenAccount.accountInfo) return true;
  }
  const bigInt = BigInt(tokenAccount.accountInfo.amount);
  console.log(bigInt);
  if (bigInt === 0n) return true;
  await sellPump(
    wallet,
    tokenAccount.accountInfo.mint,
    bigInt,
    globalAccount!,
    provider!,
    associatedCurve!,
    lastBlocks[lastBlocks.length - 1],
  );
  return false;
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
        if (gotTokenData) return;
        gotTokenData = true;
        await monitorSellLogic(mintAccount);
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

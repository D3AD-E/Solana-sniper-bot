import { TokenAmount, Token, TokenAccount, TOKEN_PROGRAM_ID, LiquidityStateV4 } from '@raydium-io/raydium-sdk';
import {
  AccountLayout,
  createBurnCheckedInstruction,
  createCloseAccountInstruction,
  getAssociatedTokenAddress,
  getMint,
} from '@solana/spl-token';
import {
  Commitment,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
  Transaction,
  TransactionConfirmationStrategy,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import { buy, getTokenAccounts } from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { RAYDIUM_LIQUIDITY_PROGRAM_ID_V4, regeneratePoolKeys } from './cryptoQueries/raydiumSwapUtils/liquidity';
import WebSocket from 'ws';
import { sendMessage } from './telegramBot';
import { helius } from './helius';
import { Block } from './listener.types';
import { isNumberInRange } from './utils/mathUtils';
import { WorkerPool } from './workers/pool';
import { envVarToBoolean } from './utils/envUtils';
import { GlobalAccount, PumpFunSDK } from 'pumpdotfun-sdk';
import { AnchorProvider, BN, Wallet } from '@coral-xyz/anchor';
import { buyPump, sellPump } from './pumpFun';
import Client, { CommitmentLevel, SubscribeRequest } from '@triton-one/yellowstone-grpc';
import { decodeData } from './decoder';
import { SYSTEM_INSTRUCTION_LAYOUTS } from './decoder/decoder.types';
import { bs58 } from '@coral-xyz/anchor/dist/cjs/utils/bytes';
import { JitoClient } from './jito/searcher';
let existingTokenAccounts: TokenAccount[] = [];

const quoteToken = Token.WSOL;
let swapAmount: TokenAmount;
let quoteTokenAssociatedAddress: PublicKey;
let ws: WebSocket | undefined = undefined;

let lastBlocks: Block[] = [];
let processedTokens: string[] = [];
let workerPool: WorkerPool | undefined = undefined;
const enableProtection = envVarToBoolean(process.env.ENABLE_PROTECTION);
let isProcessing = false;
const getProvider = () => {
  const walletAnchor = new Wallet(wallet);
  const provider = new AnchorProvider(solanaConnection, walletAnchor, {
    commitment: process.env.COMMITMENT as Commitment,
  });
  return provider;
};
let wsPairs: WebSocket | undefined = undefined;

let sdk: PumpFunSDK | undefined = undefined;
let lastRequest: any = undefined;
let gotTokenData = false;
let mintAccount = '';
let globalAccount: GlobalAccount | undefined = undefined;
let provider: AnchorProvider | undefined = undefined;
let associatedCurve: PublicKey | undefined = undefined;
let isSelling = false;
let buyAmountSol: bigint | undefined = undefined;
let buyAmount: bigint | undefined = undefined;
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
      buyAmount!,
      globalAccount!,
      provider!,
      curve,
      lastBlocks[lastBlocks.length - 1],
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

export default async function snipe(): Promise<void> {
  setInterval(storeRecentBlockhashes, 700);

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
  buyAmount = globalAccount.getInitialBuyPrice(buyAmountSol);

  // Call the subscription function
  subscribeToSlotUpdates();
  // let bondingCurveAccount = await sdj.buy(mint, commitment);

  // const tokenAccount = existingTokenAccounts.find(
  //   (acc) => acc.accountInfo.mint.toString() === '2PfSJLcibNM7CZnh3wBtiUkkXfpNNcHiz4DjZYZCpump',
  // )!;

  // const bytes = bs58.decode(
  //   '2K7nL28PxCW8ejnyCeuMpbWQwqKtqyArGAa7ccsDMsh2CEG4wEULZwVYVJm62YZ81jBFyKhYgNkYKUrXyNxAszvpk1sGQ1tBToSJUWhohnPxXTEMKMgz6U6PUtyCiPmYoAbSVdQEPBTnsTHhWkg4kiGSKScqD8sof8TKtm8PTNqdQBQYGZiCAbufFMYT',
  // );
  // const buf = Buffer.from(bytes);
  // const parsed = decodeInstructionData(buf);

  // console.log(parsed);
  // lastRequest = {
  //   jsonrpc: '2.0',
  //   id: 420,
  //   method: 'transactionSubscribe',
  //   params: [
  //     {
  //       vote: false,
  //       failed: false,
  //       accountInclude: ['6NUafpRndeekVpusnVK7DunCggjUrhDhsXHD2U5tpump'],
  //     },
  //     {
  //       commitment: 'processed',
  //       encoding: 'jsonParsed',
  //       transactionDetails: 'full',
  //       showRewards: false,
  //       maxSupportedTransactionVersion: 1,
  //     },
  //   ],
  // };
  // setupPairSocket();
  // workerPool = new WorkerPool(Number(process.env.WORKER_AMOUNT!), quoteTokenAssociatedAddress);

  // existingTokenAccounts = await getTokenAccounts(
  //   solanaConnection,
  //   wallet.publicKey,
  //   process.env.COMMITMENT as Commitment,
  // );
  // const tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === mintAccount)!;
  // console.log(tokenAccount.accountInfo.amount);
  // const bigInt = BigInt(tokenAccount.accountInfo.amount);
  // const sellResults = await sdk!.sell(wallet, new PublicKey(mintAccount), bigInt, 500000000000n, {
  //   unitLimit: 100000,
  //   unitPrice: 500000,
  // });
  // console.log(sellResults);
  // return;
  // setInterval(storeRecentBlockhashes, 100);
  // let boughtAmount: bigint = 0n;
  // logger.info('Started listening');
  // let tradeEvent = sdk!.addEventListener('tradeEvent', async (event, _, signature) => {
  //   if (event.mint.toString() === mintAccount) {
  //     if (event.user.toString() === wallet.publicKey.toString()) return;
  //     if (isSelling) return;
  //     logger.info(signature);
  //     console.log('tradeEvent', event);
  //     if (!gotTokenData) return;
  //     // const price = event.tokenAmount / event.solAmount;
  //     // if (!initialPrice) {
  //     //   initialPrice = price;
  //     //   logger.info('initial');
  //     //   logger.info(initialPrice.toString());
  //     //   return;
  //     // }
  //     // logger.info(price.toString());
  //     // const priceNumber = Number(price.toString());
  //     // const initialPriceNumber = Number(initialPrice.toString());
  //     boughtAmount = boughtAmount + (event.isBuy ? event.solAmount : -event.solAmount);
  //     // const percentageGain = ((initialPriceNumber - priceNumber) / initialPriceNumber) * 100;
  //     console.log(boughtAmount);
  //     logger.info('Change');
  //     // logger.info(percentageGain.toFixed(4));
  //     if (boughtAmount > 100000000n || boughtAmount < -1n) {
  //       //0.1 sol
  //       if (isSelling) return;
  //       while (true) {
  //         isSelling = true;
  //         try {
  //           const wasSellDone = await sellToken();
  //           if (wasSellDone) return;
  //           await new Promise((resolve) => setTimeout(resolve, 1500));
  //         } catch (e) {
  //           console.log(e);
  //           await new Promise((resolve) => setTimeout(resolve, 50));
  //         }
  //       }
  //     }
  //   }
  // });
  // console.log('tradeEvent', tradeEvent);
  // setupLiquiditySocket();
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

async function sellToken() {
  console.log('Selling');
  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );
  const tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === mintAccount)!;
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

function setupLiquiditySocket() {
  ws = new WebSocket(process.env.GEYSER_ENDPOINT!);
  ws!.on('open', function open() {
    logger.info('Listening to geyser liquidity');
    const request = {
      jsonrpc: '2.0',
      id: 420,
      method: 'transactionSubscribe',
      params: [
        {
          vote: false,
          failed: false,
          accountRequired: [
            '6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P',
            'metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s',
            'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA',
          ],
        },
        {
          commitment: 'processed',
          encoding: 'jsonParsed',
          transactionDetails: 'full',
          showRewards: false,
          maxSupportedTransactionVersion: 0,
        },
      ],
    };
    ws!.send(JSON.stringify(request));
  });
  ws!.on('message', async function incoming(data) {
    if (isProcessing) return;
    logger.info('here');
    const messageStr = data.toString();
    var jData = JSON.parse(messageStr);
    const isntructions = jData?.params?.result?.transaction?.meta?.innerInstructions;
    if (!isntructions) return;
    const instructionWithCurve =
      isntructions.find((x: any) => x.index === 5) ?? isntructions.find((x: any) => x.index === 4);
    console.log(!instructionWithCurve);
    if (!instructionWithCurve) return;
    const curve = instructionWithCurve.instructions[0].parsed.info.source;
    console.log(curve);
    const inner = isntructions[0].instructions;
    const mint = inner[0].parsed.info.newAccount;
    logger.info(mint);
    mintAccount = mint.toString();
    isProcessing = true;
    // for (let i = 0; i < 5; ++i) {
    // const result = await buyPump(
    //   wallet,
    //   new PublicKey(mint),
    //   BigInt(Number(process.env.SWAP_SOL_AMOUNT!) * LAMPORTS_PER_SOL),
    //   globalAccount!,
    //   provider!,
    //   new PublicKey(curve),
    //   maxLamports,
    //   lastBlocks[lastBlocks.length - 1],
    // );
    // console.log(result);
    // // await new Promise((resolve) => setTimeout(resolve, 100));
    // // }
    // mintAccount = mint.toString();
    // solanaConnection
    //   .confirmTransaction(result as TransactionConfirmationStrategy, 'finalized')
    //   .then(async (confirmation) => {})
    //   .catch((e) => {
    //     console.log(e);
    //     logger.warn('Buy TX hash expired');
    //     isProcessing = false;
    //   });
    return;

    // try {

    // } catch (e) {
    //   console.log(messageStr);
    //   console.error('Failed to parse JSON:', e);
    //   ws?.close();
    // }
  });
  ws!.on('error', function error(err) {
    console.error('WebSocket error:', err);
  });
  ws!.on('close', async function close() {
    logger.warn('WebSocket is closed liquidity');
    await new Promise((resolve) => setTimeout(resolve, 200));
    setupLiquiditySocket();
  });
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
      // if (updatedAccountInfo.accountId.equals(quoteTokenAssociatedAddress)) {
      //   return;
      // }
      console.log(accountData.mint.toString() === mintAccount);
      if (accountData.mint.toString() === mintAccount) {
        gotTokenData = true;
        logger.info(`Monitoring`);
        console.log(accountData.mint);
      }

      setTimeout(async () => {
        logger.info('Timeout');
        await sellToken();
      }, 7777);
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

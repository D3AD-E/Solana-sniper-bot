import { TokenAmount, Token, TokenAccount, TOKEN_PROGRAM_ID, LiquidityStateV4 } from '@raydium-io/raydium-sdk';
import { AccountLayout, getMint } from '@solana/spl-token';
import { Commitment, PublicKey, TransactionConfirmationStrategy } from '@solana/web3.js';
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
import { PumpFunSDK } from 'pumpdotfun-sdk';
let existingTokenAccounts: TokenAccount[] = [];

const quoteToken = Token.WSOL;
let swapAmount: TokenAmount;
let quoteTokenAssociatedAddress: PublicKey;
let ws: WebSocket | undefined = undefined;

const maxLamports = 500000;
let currentLamports = maxLamports;
let lastBlocks: Block[] = [];
let processedTokens: string[] = [];
let workerPool: WorkerPool | undefined = undefined;
const enableProtection = envVarToBoolean(process.env.ENABLE_PROTECTION);
const minPoolSize = 100;
export default async function snipe(): Promise<void> {
  // let bondingCurveAccount = await sdj.buy(mint, commitment);
  // existingTokenAccounts = await getTokenAccounts(
  //   solanaConnection,
  //   wallet.publicKey,
  //   process.env.COMMITMENT as Commitment,
  // );
  // const tokenAccount = existingTokenAccounts.find(
  //   (acc) => acc.accountInfo.mint.toString() === quoteToken.mint.toString(),
  // )!;
  // if (!tokenAccount) {
  //   throw new Error(`No ${quoteToken.symbol} token account found in wallet: ${wallet.publicKey}`);
  // }
  // quoteTokenAssociatedAddress = tokenAccount.pubkey;
  // workerPool = new WorkerPool(Number(process.env.WORKER_AMOUNT!), quoteTokenAssociatedAddress);
  // if (enableProtection) {
  //   setInterval(storeRecentBlockhashes, 700);
  //   await new Promise((resolve) => setTimeout(resolve, 140000));
  // } else {
  //   await new Promise((resolve) => setTimeout(resolve, 1000));
  // }
  logger.info('Started listening');

  //https://solscan.io/tx/Kvu4Qd5RBjUDoX5yzUNNtd17Bhb78qTo93hqYgDEr8hb1ysTf9zGFDgvS1QTnz6ghY3f6Fo59GWYQSgkTJxo9Cd mintundefined
  // skipped https://www.dextools.io/app/en/solana/pair-explorer/HLBmAcU65tm999f3WrshSdeFgAbZNxEGrqD6DzAdR1iF?t=1717674277533 because of jitotip, not sure if want to fix
  // setupLiquiditySocket();
  // setInterval(
  //   () => {
  //     ws?.close();
  //   },
  //   10 * 60 * 1000,
  // ); // 10 minutes
  // // await updateLamports();
  // // setInterval(updateLamports, 15000);
  // logger.info(`Wallet Address: ${wallet.publicKey}`);
  // swapAmount = new TokenAmount(Token.WSOL, process.env.SWAP_SOL_AMOUNT, false);
  // logger.info(`Swap sol amount: ${swapAmount.toFixed()} ${quoteToken.symbol}`);
  // await listenToChanges();
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
          commitment: 'singleGossip',
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
    const messageStr = data.toString();
    console.log(messageStr);
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

export async function processGeyserLiquidity(
  id: PublicKey,
  poolState: LiquidityStateV4,
  mint: PublicKey,
): Promise<TransactionConfirmationStrategy> {
  let block = undefined;
  block = await solanaConnection.getLatestBlockhash('processed');

  const packet = await buy(id, poolState, quoteTokenAssociatedAddress, currentLamports, mint, block);
  workerPool!.addTokenAccount(mint.toString(), packet.tokenAccount);
  return {
    signature: packet.signature,
    blockhash: packet.blockhash,
    lastValidBlockHeight: packet.lastValidBlockHeight,
  };
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
      if (updatedAccountInfo.accountId.equals(quoteTokenAssociatedAddress)) {
        return;
      }
      if (!workerPool!.doesTokenExist(accountData.mint.toString())) {
        logger.warn('Got unknown token in wallet');
        return;
      }
      logger.info(`Monitoring`);
      console.log(accountData.mint);
      processedTokens.push(accountData.mint.toString());
      workerPool!.gotWalletToken(accountData.mint.toString(), accountData);
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

import { TokenAmount, Token, TokenAccount, TOKEN_PROGRAM_ID, LiquidityStateV4 } from '@raydium-io/raydium-sdk';
import { AccountLayout, getMint } from '@solana/spl-token';
import { Commitment, PublicKey, TransactionConfirmationStrategy } from '@solana/web3.js';
import { buy, getTokenAccounts } from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { RAYDIUM_LIQUIDITY_PROGRAM_ID_V4 } from './cryptoQueries/raydiumSwapUtils/liquidity';
import WebSocket from 'ws';
import { sendMessage } from './telegramBot';
import { helius } from './helius';
import { Block } from './listener.types';
import { isNumberInRange } from './utils/mathUtils';
import { WorkerPool } from './workers/pool';
let existingTokenAccounts: TokenAccount[] = [];

const quoteToken = Token.WSOL;
let swapAmount: TokenAmount;
let quoteTokenAssociatedAddress: PublicKey;
let ws: WebSocket | undefined = undefined;
const timoutSec = Number(process.env.SELL_TIMEOUT_SEC!);

const maxLamports = 1200010;
let currentLamports = maxLamports;
let lastBlocks: Block[] = [];
let processedTokens: string[] = [];
let workerPool: WorkerPool | undefined = undefined;
export default async function snipe(): Promise<void> {
  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );
  const tokenAccount = existingTokenAccounts.find(
    (acc) => acc.accountInfo.mint.toString() === quoteToken.mint.toString(),
  )!;
  if (!tokenAccount) {
    throw new Error(`No ${quoteToken.symbol} token account found in wallet: ${wallet.publicKey}`);
  }
  quoteTokenAssociatedAddress = tokenAccount.pubkey;
  workerPool = new WorkerPool(2, quoteTokenAssociatedAddress);
  setInterval(storeRecentBlockhashes, 700);
  await new Promise((resolve) => setTimeout(resolve, 140000));
  logger.info('Started listening');
  //https://solscan.io/tx/Kvu4Qd5RBjUDoX5yzUNNtd17Bhb78qTo93hqYgDEr8hb1ysTf9zGFDgvS1QTnz6ghY3f6Fo59GWYQSgkTJxo9Cd mintundefined
  // skipped https://www.dextools.io/app/en/solana/pair-explorer/HLBmAcU65tm999f3WrshSdeFgAbZNxEGrqD6DzAdR1iF?t=1717674277533 because of jitotip, not sure if want to fix
  setupLiquiditySocket();
  await updateLamports();
  setInterval(updateLamports, 15000);
  logger.info(`Wallet Address: ${wallet.publicKey}`);
  swapAmount = new TokenAmount(Token.WSOL, process.env.SWAP_SOL_AMOUNT, false);
  logger.info(`Swap sol amount: ${swapAmount.toFixed()} ${quoteToken.symbol}`);
  await listenToChanges();
}

async function getFinalizedBlockheight(): Promise<number> {
  const currentSlot = await solanaConnection.getSlot('finalized');
  let block = undefined;
  for (let i = 0; i < 5; i += 1) {
    try {
      block = (await solanaConnection.getBlock(currentSlot - 2, {
        transactionDetails: 'none',
        maxSupportedTransactionVersion: 0,
      })) as any;
      break;
    } catch (e) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  if (block === undefined) throw 'Could not fetch block';
  return block.blockHeight;
}
async function getBlockForBuy() {
  const currentHeight = await getFinalizedBlockheight();
  const lastBlock = lastBlocks[lastBlocks.length - 1];
  const diff = lastBlock.lastValidBlockHeight - currentHeight;
  const min = lastBlock.lastValidBlockHeight - diff * 0.28;
  const max = lastBlock.lastValidBlockHeight - diff * 0.2;

  const block = lastBlocks.find((x) => isNumberInRange(x.lastValidBlockHeight, min, max));
  return block!;
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

async function updateLamports() {
  const prices = await helius.rpc.getPriorityFeeEstimate({
    accountKeys: ['675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8'],
    options: { includeAllPriorityFeeLevels: true },
  });
  if (prices.priorityFeeLevels?.high === undefined || maxLamports < prices.priorityFeeLevels?.high) {
    currentLamports = maxLamports;
  } else {
    currentLamports = Math.floor(prices.priorityFeeLevels?.high + 10000);
  }
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
            'ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL',
            RAYDIUM_LIQUIDITY_PROGRAM_ID_V4.toString(),
            'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA',
            'srmqPvymJeFKQ4zGQed1GFppgkRHL9kaELCbyksJtPX',
            '9DCxsMizn3H1hprZ7xWe6LDzeUeZBksYFpBWBtSf1PQX',
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
    const messageStr = data.toString();
    try {
      var jData = JSON.parse(messageStr);
      const isntructions = jData?.params?.result?.transaction?.meta?.innerInstructions;
      if (isntructions && isntructions.length === 1) {
        const inner = isntructions[0].instructions;
        if (inner[inner.length - 1]?.parsed?.type === 'mintTo') {
          const mint1 = inner[12];
          const mint2 = inner[16];
          const isFirstMintSol = mint1.parsed.info.mint === 'So11111111111111111111111111111111111111112';
          const mintAddress = isFirstMintSol ? mint2.parsed.info.mint : mint1.parsed.info.mint;
          logger.info('Mint ' + mintAddress);
          let mintAccount = undefined;
          for (let i = 0; i < 20; i += 1) {
            try {
              mintAccount = await getMint(
                solanaConnection,
                new PublicKey(mintAddress),
                process.env.COMMITMENT as Commitment,
              );
              if (mintAccount) break;
            } catch (e) {
              await new Promise((resolve) => setTimeout(resolve, 250));
            }
          }
          if (!mintAccount) throw 'Failed to get mint ';

          if (mintAccount.freezeAuthority !== null) {
            logger.warn('Token can be frozen, skipping');
            return;
          }
          if (!workerPool!.areThereFreeWorkers()) {
            logger.warn('No workers available');
            return;
          }
          logger.info('Listening to geyser Pair');
          const lastRequest = {
            jsonrpc: '2.0',
            id: 420,
            method: 'transactionSubscribe',
            params: [
              {
                vote: false,
                failed: false,
                accountInclude: [mintAddress],
              },
              {
                commitment: 'processed',
                encoding: 'jsonParsed',
                transactionDetails: 'full',
                showRewards: false,
                maxSupportedTransactionVersion: 1,
              },
            ],
          };
          workerPool!.gotToken(mintAddress, lastRequest);

          const sampleKeys = {
            status: undefined,
            owner: new PublicKey('So11111111111111111111111111111111111111112'),
            nonce: undefined,
            maxOrder: undefined,
            depth: undefined,
            baseDecimal: Number(mintAccount.decimals),
            quoteDecimal: 9,
            state: undefined,
            resetFlag: undefined,
            minSize: undefined,
            volMaxCutRatio: undefined,
            amountWaveRatio: undefined,
            baseLotSize: undefined,
            quoteLotSize: undefined,
            minPriceMultiplier: undefined,
            maxPriceMultiplier: undefined,
            systemDecimalValue: undefined,
            minSeparateNumerator: undefined,
            minSeparateDenominator: undefined,
            tradeFeeNumerator: undefined,
            tradeFeeDenominator: undefined,
            pnlNumerator: undefined,
            pnlDenominator: undefined,
            swapFeeNumerator: undefined,
            swapFeeDenominator: undefined,
            baseNeedTakePnl: undefined,
            quoteNeedTakePnl: undefined,
            quoteTotalPnl: undefined,
            baseTotalPnl: undefined,
            poolOpenTime: undefined,
            punishPcAmount: undefined,
            punishCoinAmount: undefined,
            orderbookToInitTime: undefined,
            swapBaseInAmount: undefined,
            swapQuoteOutAmount: undefined,
            swapBase2QuoteFee: undefined,
            swapQuoteInAmount: inner[30].parsed.info.amount,
            swapBaseOutAmount: undefined,
            swapQuote2BaseFee: undefined,
            baseVault: new PublicKey(inner[12].parsed.info.account),
            quoteVault: new PublicKey(inner[15].parsed.info.account),
            baseMint: isFirstMintSol
              ? new PublicKey('So11111111111111111111111111111111111111112')
              : new PublicKey(mintAddress),
            quoteMint: isFirstMintSol
              ? new PublicKey(mintAddress)
              : new PublicKey('So11111111111111111111111111111111111111112'), //was q sol
            lpMint: new PublicKey(inner[31].parsed.info.mint),
            openOrders: new PublicKey(inner[22].parsed.info.account),
            marketId: new PublicKey(inner[23].accounts[2]),
            marketProgramId: new PublicKey(inner[23].programId),
            targetOrders: new PublicKey(inner[4].parsed.info.account),
            withdrawQueue: new PublicKey('11111111111111111111111111111111'),
            lpVault: new PublicKey('11111111111111111111111111111111'),
            lpReserve: undefined,
            padding: [],
          };
          try {
            const packet = await processGeyserLiquidity(
              new PublicKey(inner[inner.length - 13].parsed.info.account),
              sampleKeys,
              new PublicKey(mintAddress),
            );
            solanaConnection
              .confirmTransaction(packet as TransactionConfirmationStrategy, 'finalized')
              .then(async (confirmation) => {
                if (confirmation.value.err) {
                  logger.warn('Sent buy bundle but it failed');
                  workerPool!.freeWorker(mintAddress);
                  ws?.close();
                } else {
                  await new Promise((resolve) => setTimeout(resolve, 5000));
                  if (!processedTokens.some((t) => t === mintAddress)) {
                    logger.warn('Websocket took too long');
                    const tokenAccounts = await getTokenAccounts(solanaConnection, wallet.publicKey, 'processed');
                    await new Promise((resolve) => setTimeout(resolve, 500));
                    logger.info('Currentkey ' + mintAddress);
                    const tokenAccount = tokenAccounts.find((acc) => acc.accountInfo.mint.toString() === mintAddress)!;
                    if (!tokenAccount) {
                      logger.info(`No token account found in wallet, but it succeeded`);
                      return;
                    }
                    logger.warn(`Selling bc didnt get token`);
                    workerPool!.forceSell(mintAddress, {
                      ...tokenAccount.accountInfo,
                      delegateOption: tokenAccount.accountInfo.delegateOption === 1 ? 1 : 0,
                      isNativeOption: tokenAccount.accountInfo.isNativeOption === 1 ? 1 : 0,
                      closeAuthorityOption: tokenAccount.accountInfo.closeAuthorityOption === 1 ? 1 : 0,
                    });
                  }
                }
              })
              .catch((e) => {
                console.log(e);
                logger.warn('TX hash expired, hopefully we didnt crash');
                workerPool!.freeWorker(mintAddress);
                ws?.close();
              });
          } catch (e) {
            logger.warn('Buy failed');
            console.error(e);
            workerPool!.freeWorker(mintAddress);
          }
        }
      }
    } catch (e) {
      console.log(messageStr);
      console.error('Failed to parse JSON:', e);
      ws?.close();
    }
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
  const poolSize = new TokenAmount(quoteToken, poolState.swapQuoteInAmount, true);
  logger.info(`Processing pool: ${mint.toString()} with ${poolSize.toFixed()} ${quoteToken.symbol} in liquidity`);
  if (Number(poolSize.toFixed()) < 0.4) throw 'Pool too low';
  const block = await getBlockForBuy();
  logger.info(`Got block`);

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
      const timeToSellTimeoutGeyser = new Date();
      timeToSellTimeoutGeyser.setTime(timeToSellTimeoutGeyser.getTime() + timoutSec * 1000);
      workerPool!.gotWalletToken(accountData.mint.toString(), timeToSellTimeoutGeyser, accountData);
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

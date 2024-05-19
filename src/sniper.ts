import {
  TokenAmount,
  Token,
  TokenAccount,
  MARKET_STATE_LAYOUT_V3,
  TOKEN_PROGRAM_ID,
  LiquidityStateV4,
  MarketStateV3,
} from '@raydium-io/raydium-sdk';
import { AccountLayout, RawAccount, getMint } from '@solana/spl-token';
import { Commitment, KeyedAccountInfo, PublicKey } from '@solana/web3.js';
import { buyJito, getTokenAccounts, saveTokenAccount, sellJito } from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { LEADERS_FILE_NAME } from './constants';
import { OPENBOOK_PROGRAM_ID, RAYDIUM_LIQUIDITY_PROGRAM_ID_V4 } from './cryptoQueries/raydiumSwapUtils/liquidity';
import { readFile, writeFile } from 'fs/promises';
import { getTokenPrice } from './birdEye';
import { BundlePacket } from './listener.types';
import { MinimalTokenAccountData } from './cryptoQueries/cryptoQueries.types';
import { JitoClient } from './jito/searcher';
import BigNumber from 'bignumber.js';
import { SearcherClient } from 'jito-ts/dist/sdk/block-engine/searcher';
import WebSocket from 'ws';
import { SlotList } from 'jito-ts/dist/gen/block-engine/searcher';
import { refreshLeaders, readLeaders } from './jito/leaders';
let existingTokenAccounts: TokenAccount[] = [];

const quoteToken = Token.WSOL;
let swapAmount: TokenAmount;
let quoteTokenAssociatedAddress: PublicKey;
const existingLiquidityPools: Set<string> = new Set<string>();
const existingTokenAccountsExtended: Map<string, MinimalTokenAccountData> = new Map<string, MinimalTokenAccountData>();
const existingOpenBookMarkets: Set<string> = new Set<string>();
let client: SearcherClient | undefined = undefined;
let bundleIdProcessQueue: BundlePacket[] = [];
let currentTokenKey = '';
let bignumberInitialPrice: BigNumber | undefined = undefined;
let wsPairs: WebSocket | undefined = undefined;
let ws: WebSocket | undefined = undefined;
const stopLossPrecents = Number(process.env.STOP_LOSS_PERCENTS!) * -1;
const takeProfitPercents = Number(process.env.TAKE_PROFIT_PERCENTS!);
const blocksMin = Number(process.env.BLOCKS_MIN_LIMIT!);
const blocksMax = Number(process.env.BLOCKS_MAX_LIMIT!);
const timoutSec = Number(process.env.SELL_TIMEOUT_SEC!);
let processingToken = false;
let gotWalletToken = false;

let foundTokenData: RawAccount | undefined = undefined;
let timeToSellTimeoutGeyser: Date | undefined = undefined;
let currentSuddenPriceIncrease = 0;
let leaders:
  | {
      [key: string]: SlotList;
    }
  | undefined = undefined;
export default async function snipe(): Promise<void> {
  await refreshLeaders();
  leaders = await readLeaders();
  setupPairSocket();
  setupLiquiditySocket();

  // const tips = new WebSocket('ws://bundles-api-rest.jito.wtf/api/v1/bundles/tip_stream');

  // tips.on('open', function open() {});
  // tips.on('message', async function incoming(data) {
  //   const messageStr = data.toString();
  //   try {
  //     var jData = JSON.parse(messageStr);
  //     console.log(messageStr);
  //   } catch (e) {
  //     console.error('Failed to parse JSON:', e);
  //   }
  // });

  logger.info(`Wallet Address: ${wallet.publicKey}`);
  swapAmount = new TokenAmount(Token.WSOL, process.env.SWAP_SOL_AMOUNT, false);
  logger.info(`Swap sol amount: ${swapAmount.toFixed()} ${quoteToken.symbol}`);
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
  client = await JitoClient.getInstance();
  client.onBundleResult(
    async (bundleResult) => {
      const bundlePacket = bundleIdProcessQueue.find((x) => x.bundleId === bundleResult.bundleId);
      if (bundlePacket !== undefined) {
        console.log('result:', bundleResult);
        logger.info('Res');
        console.log(
          bundleResult.rejected?.simulationFailure?.msg?.endsWith('Blockhash not found]') &&
            bundlePacket.failAction !== undefined,
          bundleResult.rejected?.simulationFailure?.msg?.endsWith('Blockhash not found]'),
          bundlePacket.failAction !== undefined,
        );
        if (
          bundleResult.rejected?.simulationFailure?.msg?.endsWith('Blockhash not found]') &&
          bundlePacket.failAction !== undefined
        ) {
          await new Promise((resolve) => setTimeout(resolve, 3000));
          bundlePacket.failAction();
          await new Promise((resolve) => setTimeout(resolve, 3000));
          bundlePacket.failAction();
          await new Promise((resolve) => setTimeout(resolve, 3000));
          bundlePacket.failAction();
          return;
        }
        bundleIdProcessQueue = bundleIdProcessQueue.filter((item) => item.bundleId !== bundlePacket.bundleId);
        return bundleResult.bundleId;
      }
    },
    (e) => {
      logger.warn('Error');
      console.error(e);
      // if (retry) retry();
      // throw e;
    },
  );
  await listenToChanges();
}

function setupLiquiditySocket() {
  ws = new WebSocket(process.env.GEYSER_ENDPOINT!);
  ws.on('open', function open() {
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
  ws.on('message', async function incoming(data) {
    if (processingToken) return;
    const messageStr = data.toString();
    try {
      var jData = JSON.parse(messageStr);
      const isntructions = jData?.params?.result?.transaction?.meta?.innerInstructions;
      if (isntructions && isntructions.length === 1) {
        const inner = isntructions[0].instructions;
        if (inner[inner.length - 1]?.parsed?.type === 'mintTo') {
          if (processingToken) return;
          processingToken = true;
          const mint1 = inner[12];
          const mint2 = inner[16];
          const isFirstMintSol = mint1.parsed.info.mint === 'So11111111111111111111111111111111111111112';
          const mintAddress = isFirstMintSol ? mint2.parsed.info.mint : mint1.parsed.info.mint;
          logger.info(mintAddress);
          const mintAccount = await getMint(
            solanaConnection,
            new PublicKey(mintAddress),
            process.env.COMMITMENT as Commitment,
          );
          if (mintAccount.freezeAuthority !== null) {
            logger.warn('Token can be frozen, skipping');
            processingToken = false;
            return;
          }
          const isBlockSuitable = await isNextAuctionBlockSoon();
          if (!isBlockSuitable) {
            logger.warn('Block is too far');
            processingToken = false;
            return;
          }
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
            baseVault: new PublicKey(isFirstMintSol ? inner[15].parsed.info.account : inner[12].parsed.info.account),
            quoteVault: new PublicKey(isFirstMintSol ? inner[12].parsed.info.account : inner[15].parsed.info.account),
            baseMint: new PublicKey(mintAddress),
            quoteMint: new PublicKey('So11111111111111111111111111111111111111112'),
            lpMint: new PublicKey(inner[31].parsed.info.mint),
            openOrders: new PublicKey(inner[22].parsed.info.account),
            marketId: new PublicKey(inner[23].accounts[2]), //somitemes it breaks?
            marketProgramId: new PublicKey(inner[23].programId),
            targetOrders: new PublicKey(inner[4].parsed.info.account),
            withdrawQueue: new PublicKey('11111111111111111111111111111111'),
            lpVault: new PublicKey('11111111111111111111111111111111'),
            lpReserve: undefined,
            padding: [],
          };
          currentTokenKey = mintAddress;
          try {
            const packet = await processGeyserLiquidity(
              new PublicKey(inner[inner.length - 13].parsed.info.account),
              sampleKeys,
            );
            if (!packet) {
              logger.warn('Leader too far');
              processingToken = false;
              return;
            }
            bundleIdProcessQueue.push({ bundleId: packet!, failAction: undefined });
          } catch (e) {
            logger.warn('Buy failed');
            console.error(e);
            processingToken = false;
            return;
          }
          logger.info('Listening to geyser Pair');
          const request = {
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
          wsPairs!.send(JSON.stringify(request));
          const timeout = setTimeout(() => {
            if (processingToken && !gotWalletToken) {
              logger.warn('Send buy bundle but if failed');
              processingToken = false;
            }
          }, 30000);
        }
      }
    } catch (e) {
      console.error('Failed to parse JSON:', e);
    }
  });
  ws.on('error', function error(err) {
    console.error('WebSocket error:', err);
  });
  ws.on('close', function close() {
    console.log('WebSocket is closed');
  });
}

function setupPairSocket() {
  wsPairs = new WebSocket(process.env.GEYSER_ENDPOINT!);
  wsPairs.on('open', function open() {});
  wsPairs.on('message', async function incoming(data) {
    const messageStr = data.toString();
    try {
      var jData = JSON.parse(messageStr);
      const instructionWithSwapSell = jData?.params?.result?.transaction?.meta?.innerInstructions[0];
      if (instructionWithSwapSell !== undefined) {
        getSwappedAmounts(instructionWithSwapSell);
      }
      const instructionWithSwapBuy =
        jData?.params?.result?.transaction?.meta?.innerInstructions[
          jData?.params?.result?.transaction?.meta?.innerInstructions.length - 1
        ];
      if (instructionWithSwapBuy !== undefined) {
        getSwappedAmounts(instructionWithSwapBuy);
      }
      const instructionWithSwapBuy2 =
        jData?.params?.result?.transaction?.meta?.innerInstructions[
          jData?.params?.result?.transaction?.meta?.innerInstructions.length - 2
        ];
      if (instructionWithSwapBuy2 !== undefined) {
        getSwappedAmounts(instructionWithSwapBuy2);
      }
    } catch (e) {
      console.error('Failed to parse JSON:', e);
    }
  });
  wsPairs.on('error', function error(err) {
    console.error('WebSocket error:', err);
  });
  wsPairs.on('close', function close() {
    console.log('WebSocket is closed');
  });
}

async function isNextAuctionBlockSoon() {
  const j = await JitoClient.getInstance();
  const b = await j.getNextScheduledLeader();

  const currentBlock = b.currentSlot;
  console.log(currentBlock);
  for (const l in leaders) {
    const leader = leaders[l];
    const block = leader.slots.find((s) => s - currentBlock <= blocksMax && s - currentBlock > blocksMin);
    if (block) {
      console.log(block, l);
      return true;
    }
  }
  return false;
}

function getSwappedAmounts(instructionWithSwap: any) {
  const swapDataBuy = instructionWithSwap.instructions?.filter((x: any) => x.parsed?.info.amount !== undefined);
  if (swapDataBuy !== undefined) {
    const sol = swapDataBuy.find(
      (x: any) => x.parsed.info.authority !== '5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1',
    );
    if (sol) {
      const other = swapDataBuy.find(
        (x: any) => x.parsed.info.authority === '5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1',
      );
      const price = BigNumber(sol.parsed.info.amount as string).div(other.parsed.info.amount as string);
      if (foundTokenData && bignumberInitialPrice) {
        const percentageGain = price.minus(bignumberInitialPrice).div(bignumberInitialPrice).multipliedBy(100);
        logger.info(percentageGain.toString());
        const percentageGainNumber = Number(percentageGain.toFixed(5));
        if (percentageGainNumber > 100) {
          currentSuddenPriceIncrease++;
        } else currentSuddenPriceIncrease = 0;
        if (percentageGainNumber <= stopLossPrecents) {
          logger.warn(`Selling at LOSS, loss ${percentageGain}%, addr ${foundTokenData!.mint.toString()}`);
          sellOnActionGeyser(foundTokenData!);
          return;
        }
        if (percentageGainNumber >= takeProfitPercents) {
          if (percentageGainNumber > 100) {
            if (currentSuddenPriceIncrease >= 8) {
              logger.info(`Selling at TAKEPROFIT, increase ${percentageGain}, addr ${foundTokenData!.mint.toString()}`);
              sellOnActionGeyser(foundTokenData!);
              return;
            } else return;
          }
          logger.info(`Selling at TAKEPROFIT, increase ${percentageGain}%, addr ${foundTokenData!.mint.toString()}`);
          sellOnActionGeyser(foundTokenData!);

          return;
        }
        if (new Date() >= timeToSellTimeoutGeyser!) {
          logger.info(`Selling at TIMEOUT, change ${percentageGain}%, addr ${foundTokenData!.mint.toString()}`);
          sellOnActionGeyser(foundTokenData!);
          return;
        }
      }
    }
  }
}

async function sellOnActionGeyser(account: RawAccount) {
  bignumberInitialPrice = undefined;
  foundTokenData = undefined;
  // gotWalletToken = false;
  // processingToken = false;
  const packet = await sellJito(
    account.mint,
    account.amount,
    existingTokenAccountsExtended,
    quoteTokenAssociatedAddress,
  );
  if (packet !== undefined) {
    bundleIdProcessQueue.push({
      bundleId: packet,
      failAction: async () => {
        const packet = await sellJito(
          account!.mint,
          account!.amount,
          existingTokenAccountsExtended,
          quoteTokenAssociatedAddress,
        );
        bundleIdProcessQueue.push({ bundleId: packet!, failAction: undefined });
      },
    });
  }
}

export async function processGeyserLiquidity(id: PublicKey, poolState: LiquidityStateV4) {
  const poolSize = new TokenAmount(quoteToken, poolState.swapQuoteInAmount, true);
  logger.info(
    `Processing pool: ${poolState.baseMint.toString()} with ${poolSize.toFixed()} ${quoteToken.symbol} in liquidity`,
  );
  const recentBlockhashForSwap = await solanaConnection.getLatestBlockhash('finalized');
  const packet = await buyJito(
    id,
    poolState,
    existingTokenAccountsExtended,
    quoteTokenAssociatedAddress,
    recentBlockhashForSwap.blockhash,
  );
  return packet;
}

export async function processOpenBookMarket(updatedAccountInfo: KeyedAccountInfo) {
  let accountData: MarketStateV3 | undefined;
  try {
    accountData = MARKET_STATE_LAYOUT_V3.decode(updatedAccountInfo.accountInfo.data);

    if (existingTokenAccountsExtended.has(accountData.baseMint.toString())) {
      return;
    }
    const token = saveTokenAccount(accountData.baseMint, accountData);
    existingTokenAccountsExtended.set(accountData.baseMint.toString(), token);
    // logger.info(accountData.baseMint.toString());
  } catch (e) {
    logger.debug(e);
  }
}
async function listenToChanges() {
  const openBookSubscriptionId = solanaConnection.onProgramAccountChange(
    OPENBOOK_PROGRAM_ID,
    async (updatedAccountInfo) => {
      const key = updatedAccountInfo.accountId.toString();
      const existing = existingOpenBookMarkets.has(key);
      if (!existing) {
        existingOpenBookMarkets.add(key);
        const _ = processOpenBookMarket(updatedAccountInfo);
      }
    },
    'singleGossip' as Commitment,
    [
      { dataSize: MARKET_STATE_LAYOUT_V3.span },
      {
        memcmp: {
          offset: MARKET_STATE_LAYOUT_V3.offsetOf('quoteMint'),
          bytes: quoteToken.mint.toBase58(),
        },
      },
    ],
  );
  const walletSubscriptionId = solanaConnection.onProgramAccountChange(
    TOKEN_PROGRAM_ID,
    async (updatedAccountInfo) => {
      const accountData = AccountLayout.decode(updatedAccountInfo.accountInfo!.data);
      if (accountData.mint.toString() === quoteToken.mint.toString()) {
        const walletBalance = new TokenAmount(Token.WSOL, accountData.amount, true);
        logger.info('WSOL amount change ' + walletBalance.toFixed(4));
        if (gotWalletToken) {
          // gotWalletToken = false;
          // processing = false;
          // price = 0;
        }
        return;
      }
      if (updatedAccountInfo.accountId.equals(quoteTokenAssociatedAddress)) {
        return;
      }
      if (gotWalletToken) return;
      gotWalletToken = true;
      logger.info(`Monitoring`);
      if (currentTokenKey !== accountData.mint.toString()) {
        logger.warn('Got unknown token in wallet');
        gotWalletToken = true;
        return;
      }
      timeToSellTimeoutGeyser = new Date();
      timeToSellTimeoutGeyser.setTime(timeToSellTimeoutGeyser.getTime() + timoutSec * 1000);
      foundTokenData = accountData;
      const solAmount = Number(process.env.SWAP_SOL_AMOUNT!);
      const solAmountBn = BigNumber(solAmount).multipliedBy('1000000000');
      bignumberInitialPrice = solAmountBn.div(BigNumber(accountData.amount.toString()));
      console.log(accountData.mint);
    },
    process.env.COMMITMENT as Commitment,
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

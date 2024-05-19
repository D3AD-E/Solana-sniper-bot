import {
  TokenAmount,
  Token,
  Liquidity,
  LiquidityPoolKeysV4,
  LiquidityPoolKeys,
  TokenAccount,
  LIQUIDITY_STATE_LAYOUT_V4,
  MARKET_STATE_LAYOUT_V3,
  SPL_MINT_LAYOUT,
  Market,
  TOKEN_PROGRAM_ID,
  LiquidityStateV4,
  MarketStateV3,
  struct,
  nu64,
  u8,
  blob,
  Price,
} from '@raydium-io/raydium-sdk';
import { AccountLayout, RawAccount, getMint } from '@solana/spl-token';
import { Commitment, Connection, KeyedAccountInfo, PublicKey } from '@solana/web3.js';
import {
  buy,
  buyJito,
  checkMintable,
  closeAccount,
  createAccount,
  getPoolKeys,
  getTokenAccounts,
  getTokenBalanceSpl,
  preformSwap,
  saveTokenAccount,
  sellJito,
} from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { LEADERS_FILE_NAME, MAX_REFRESH_DELAY, MIN_REFRESH_DELAY, TOKENS_FILE_NAME } from './constants';
import {
  OPENBOOK_PROGRAM_ID,
  RAYDIUM_LIQUIDITY_PROGRAM_ID_V4,
  findPoolInfoForTokensById,
  loadPoolKeys,
  regeneratePoolKeys,
} from './cryptoQueries/raydiumSwapUtils/liquidity';
import { existsSync } from 'fs';
import { readFile, writeFile } from 'fs/promises';
import { TokenInfo, getLastUpdatedTokens, getSwapInfo } from './browser/scrape';
import { sendMessage } from './telegramBot';
import { getTokenPrice } from './birdEye';
import { ProviderType, getProviderType } from './enums/LiqudityProviderType';
import { BoughtTokenData, BundlePacket } from './listener.types';
import bs58 from 'bs58';
import { MinimalTokenAccountData } from './cryptoQueries/cryptoQueries.types';
import { JitoClient } from './jito/searcher';
import BigNumber from 'bignumber.js';
import { SearcherClient } from 'jito-ts/dist/sdk/block-engine/searcher';
import WebSocket from 'ws';
import { SlotList } from 'jito-ts/dist/gen/block-engine/searcher';
let existingTokenAccounts: TokenAccount[] = [];

const quoteToken = Token.WSOL;
let swapAmount: TokenAmount;
let quoteTokenAssociatedAddress: PublicKey;
const existingLiquidityPools: Set<string> = new Set<string>();
const existingTokenAccountsExtended: Map<string, MinimalTokenAccountData> = new Map<string, MinimalTokenAccountData>();
const existingOpenBookMarkets: Set<string> = new Set<string>();
let client: SearcherClient | undefined = undefined;
let bundleIdProcessQueue: BundlePacket[] = [];
let price = 0;
let bignumberInitialPrice: BigNumber | undefined = undefined;
const wsPairs = new WebSocket(process.env.GEYSER_ENDPOINT!);
const stopLossPrecents = Number(process.env.STOP_LOSS_PERCENTS!) * -1;
const takeProfitPercents = Number(process.env.TAKE_PROFIT_PERCENTS!);

let processingToken = false;

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
  const ws = new WebSocket(process.env.GEYSER_ENDPOINT!);
  // //Initialize2
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
    ws.send(JSON.stringify(request));
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
          logger.info(inner.length);
          const mint1 = inner[12];
          const mint2 = inner[16];
          const isFirstMintSol = mint1.parsed.info.mint === 'So11111111111111111111111111111111111111112';
          const mintAddress = isFirstMintSol ? mint2.parsed.info.mint : mint1.parsed.info.mint;
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
            marketId: new PublicKey(inner[23].accounts[2]),
            marketProgramId: new PublicKey(inner[23].programId),
            targetOrders: new PublicKey(inner[4].parsed.info.account),
            withdrawQueue: new PublicKey('11111111111111111111111111111111'),
            lpVault: new PublicKey('11111111111111111111111111111111'),
            lpReserve: undefined,
            padding: [],
          };
          const packet = await processGeyserLiquidity(
            new PublicKey(inner[inner.length - 13].parsed.info.account),
            sampleKeys,
          );
          if (!packet) {
            logger.warn('Leader too far');
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
          wsPairs.send(JSON.stringify(request));
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

async function isNextAuctionBlockSoon() {
  const b = await solanaConnection.getSlot('processed');
  console.log(b);
  const currentBlock = b; // 1+actual
  for (const l in leaders) {
    const leader = leaders[l];
    const block = leader.slots.find((s) => s - currentBlock <= 10 && s - currentBlock > 3); //266736727 accepted at 266736730
    if (block) {
      console.log(block, l);
      return true;
    }
  }
  return false;
}

async function refreshLeaders() {
  const j = await JitoClient.getInstance();
  const l = await j.getConnectedLeaders();
  await writeFile(LEADERS_FILE_NAME, JSON.stringify(l));
}

async function readLeaders() {
  const data = JSON.parse((await readFile(LEADERS_FILE_NAME)).toString()) as {
    [key: string]: SlotList;
  };
  return data;
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

export async function processRaydiumPool(id: PublicKey, poolState: LiquidityStateV4, hash: string) {
  const poolSize = new TokenAmount(quoteToken, poolState.swapQuoteInAmount, true);
  logger.info(
    `Processing pool: ${poolState.baseMint.toString()} with ${poolSize.toFixed()} ${quoteToken.symbol} in liquidity`,
  );
  const packet = await buyJito(id, poolState, existingTokenAccountsExtended, quoteTokenAssociatedAddress, hash);
  const watchTokenAddress = poolState.baseMint.toString();
  let pricefetchTry = 0;
  await new Promise((resolve) => setTimeout(resolve, 1000));
  price = 0;
  while (price === 0) {
    price = (await getTokenPrice(watchTokenAddress)) ?? 0;
    await new Promise((resolve) => setTimeout(resolve, 500));
    pricefetchTry += 1;
    if (pricefetchTry >= 5) return packet;
  }
  return packet;
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
  const watchTokenAddress = poolState.baseMint.toString();
  let pricefetchTry = 0;
  await new Promise((resolve) => setTimeout(resolve, 1000));
  price = 0;
  while (price === 0) {
    price = (await getTokenPrice(watchTokenAddress)) ?? 0;
    await new Promise((resolve) => setTimeout(resolve, 500));
    pricefetchTry += 1;
    if (pricefetchTry >= 5) return packet;
  }
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
  let processing = false;
  let gotWalletToken = false;
  const runTimestamp = Math.floor(new Date().getTime() / 1000);

  // const raydiumSubscriptionId = solanaConnection.onProgramAccountChange(
  //   RAYDIUM_LIQUIDITY_PROGRAM_ID_V4,
  //   async (updatedAccountInfo) => {
  //     if (processing) return;
  //     const key = updatedAccountInfo.accountId.toString();
  //     const poolState = LIQUIDITY_STATE_LAYOUT_V4.decode(updatedAccountInfo.accountInfo.data);
  //     const poolOpenTime = parseInt(poolState.poolOpenTime.toString());
  //     const existing = existingLiquidityPools.has(key);

  //     if (poolOpenTime > runTimestamp && !existing) {
  //       if (processing) return;
  //       processing = true;
  //       const recentBlockhashForSwap = await solanaConnection.getLatestBlockhash('finalized');
  //       let mintAccount = await getMint(solanaConnection, poolState.baseMint, process.env.COMMITMENT as Commitment);
  //       if (mintAccount.freezeAuthority !== null) {
  //         logger.warn('Token can be frozen, skipping');
  //         processing = false;
  //         existingLiquidityPools.add(key);
  //         return;
  //       }
  //       const packet = await processRaydiumPool(
  //         updatedAccountInfo.accountId,
  //         poolState,
  //         recentBlockhashForSwap.blockhash,
  //       );
  //       existingLiquidityPools.add(key);
  //       if (!packet) {
  //         logger.warn('Leader too far');
  //         processing = false;
  //         return;
  //       }

  //       bundleIdProcessQueue.push({
  //         bundleId: packet,
  //         failAction: () => {
  //           processing = false;
  //         },
  //       });
  //     }
  //   },
  //   'finalized' as Commitment,
  //   [
  //     { dataSize: LIQUIDITY_STATE_LAYOUT_V4.span },
  //     {
  //       memcmp: {
  //         offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('quoteMint'),
  //         bytes: quoteToken.mint.toBase58(),
  //       },
  //     },
  //     {
  //       memcmp: {
  //         offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('marketProgramId'),
  //         bytes: OPENBOOK_PROGRAM_ID.toBase58(),
  //       },
  //     },
  //     {
  //       memcmp: {
  //         offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('status'),
  //         bytes: bs58.encode([6, 0, 0, 0, 0, 0, 0, 0]),
  //       },
  //     },
  //   ],
  // );

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
        if (gotWalletToken && processing) {
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
      timeToSellTimeoutGeyser = new Date();
      timeToSellTimeoutGeyser.setTime(timeToSellTimeoutGeyser.getTime() + 60 * 1000);
      foundTokenData = accountData;
      const solAmount = Number(process.env.SWAP_SOL_AMOUNT!);
      const solAmountBn = BigNumber(solAmount).multipliedBy('1000000000');
      bignumberInitialPrice = solAmountBn.div(BigNumber(accountData.amount.toString()));
      console.log(accountData.mint);
      // let priceFetchTry = 0;
      // const watchTokenAddress = accountData.mint.toString();
      // console.log(price);
      // while (price === 0) {
      //   price = (await getTokenPrice(watchTokenAddress)) ?? 0;
      //   await new Promise((resolve) => setTimeout(resolve, 500));
      //   priceFetchTry += 1;
      //   console.log(priceFetchTry);
      //   if (priceFetchTry >= 15) {
      //     logger.warn('Cannot get token price so selling');
      //     const packet = await sellJito(
      //       accountData.mint,
      //       accountData.amount,
      //       existingTokenAccountsExtended,
      //       quoteTokenAssociatedAddress,
      //     );
      //     if (packet !== undefined) {
      //       bundleIdProcessQueue.push({
      //         bundleId: packet,
      //         failAction: async () => {
      //           const packet = await sellJito(
      //             accountData.mint,
      //             accountData.amount,
      //             existingTokenAccountsExtended,
      //             quoteTokenAssociatedAddress,
      //           );
      //           bundleIdProcessQueue.push({ bundleId: packet!, failAction: undefined });
      //         },
      //       });
      //     }
      //     // gotWalletToken = false;
      //     // processing = false;
      //     // price = 0;
      //     return;
      //   }
      // }

      // monitorToken(watchTokenAddress, price, async () => {
      //   const packet = await sellJito(
      //     accountData.mint,
      //     accountData.amount,
      //     existingTokenAccountsExtended,
      //     quoteTokenAssociatedAddress,
      //   );
      //   if (packet !== undefined) {
      //     bundleIdProcessQueue.push({
      //       bundleId: packet,
      //       failAction: async () => {
      //         const packet = await sellJito(
      //           accountData.mint,
      //           accountData.amount,
      //           existingTokenAccountsExtended,
      //           quoteTokenAssociatedAddress,
      //         );
      //         bundleIdProcessQueue.push({ bundleId: packet!, failAction: undefined });
      //       },
      //     });
      //   }
      //   gotWalletToken = false;
      //   processing = false;
      //   price = 0;
      // });
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

async function monitorToken(address: string, initialPrice: number, sell: any) {
  console.log('monitorToken');
  const timeToSellTimeout = new Date();
  timeToSellTimeout.setTime(timeToSellTimeout.getTime() + 60 * 1000);
  let timeToSellTimeoutByPriceNotChanging = new Date();
  timeToSellTimeoutByPriceNotChanging.setTime(timeToSellTimeoutByPriceNotChanging.getTime() + 20 * 1000);
  let percentageGainCurrent = 0;
  const increasePriceDelay = 4;
  let currentSuddenPriceIncrease = 0;
  const megaPriceIncrease = 100;
  while (true) {
    const tokenPrice = await getTokenPrice(address);
    if (tokenPrice === undefined) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      continue;
    }
    const percentageGain = ((tokenPrice - initialPrice) / initialPrice) * 100;
    if (percentageGainCurrent !== percentageGain) {
      percentageGainCurrent = percentageGain;
      timeToSellTimeoutByPriceNotChanging = new Date();
      timeToSellTimeoutByPriceNotChanging.setTime(timeToSellTimeoutByPriceNotChanging.getTime() + 20 * 1000);
      console.log(address, percentageGain);
      if (percentageGain >= 500) {
        logger.info(`Selling at ${tokenPrice}$ STRANGEACTION, increase ${percentageGain}, addr ${address}%`);
        sell();
        return;
      }
    }
    if (percentageGain > megaPriceIncrease) {
      currentSuddenPriceIncrease++;
    } else currentSuddenPriceIncrease = 0;
    if (percentageGain <= stopLossPrecents) {
      logger.warn(`Selling at ${tokenPrice}$ LOSS, loss ${percentageGain}%, addr ${address}`);
      sell();
      return;
    }
    if (percentageGain >= takeProfitPercents) {
      if (percentageGain > megaPriceIncrease) {
        if (currentSuddenPriceIncrease >= increasePriceDelay) {
          logger.info(`Selling at ${tokenPrice}$ TAKEPROFIT, increase ${percentageGain}, addr ${address}%`);
          sell();
          return;
        } else continue;
      }
      logger.info(`Selling at ${tokenPrice}$ TAKEPROFIT, increase ${percentageGain}%, addr ${address}`);
      sell();

      return;
    }
    if (new Date() >= timeToSellTimeout || new Date() >= timeToSellTimeoutByPriceNotChanging) {
      logger.info(`Selling at ${tokenPrice}$ TIMEOUT, change ${percentageGain}%, addr ${address}`);
      sell();
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

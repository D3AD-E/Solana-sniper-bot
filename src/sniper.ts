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
import { AccountLayout, getMint } from '@solana/spl-token';
import { Commitment, Connection, KeyedAccountInfo, PublicKey } from '@solana/web3.js';
import {
  buyJito,
  checkMintable,
  closeAccount,
  createAccount,
  getPoolKeys,
  getTokenAccounts,
  getTokenBalanceSpl,
  preformSwap,
  preformSwapJito,
  saveTokenAccount,
  sellJito,
} from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { MAX_REFRESH_DELAY, MIN_REFRESH_DELAY, TOKENS_FILE_NAME } from './constants';
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
import { BoughtTokenData } from './listener.types';
import bs58 from 'bs58';
import { MinimalTokenAccountData } from './cryptoQueries/cryptoQueries.types';
import { JitoClient } from './jito/searcher';
import BigNumber from 'bignumber.js';

let existingTokenAccounts: TokenAccount[] = [];

const quoteToken = Token.WSOL;
let swapAmount: TokenAmount;
let quoteTokenAssociatedAddress: PublicKey;
const existingLiquidityPools: Set<string> = new Set<string>();
const existingTokenAccountsExtended: Map<string, MinimalTokenAccountData> = new Map<string, MinimalTokenAccountData>();
const existingOpenBookMarkets: Set<string> = new Set<string>();
let watchTokenAddress = '';
let price = 0;
export default async function snipe(): Promise<void> {
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
  await listenToChanges();
}

export async function processRaydiumPool(id: PublicKey, poolState: LiquidityStateV4) {
  const poolSize = new TokenAmount(quoteToken, poolState.swapQuoteInAmount, true);
  logger.info(`Processing pool: ${id.toString()} with ${poolSize.toFixed()} ${quoteToken.symbol} in liquidity`);
  watchTokenAddress = poolState.baseMint.toString();
  await buyJito(id, poolState, existingTokenAccountsExtended, quoteTokenAssociatedAddress);
  let pricefetchTry = 0;
  await new Promise((resolve) => setTimeout(resolve, 1000));
  while (price === 0) {
    price = (await getTokenPrice(watchTokenAddress)) ?? 0;
    logger.info(`Price, ${price}`);
    await new Promise((resolve) => setTimeout(resolve, 500));
    pricefetchTry += 1;
    if (pricefetchTry >= 5) return;
  }
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
  } catch (e) {
    logger.debug(e);
  }
}
async function listenToChanges() {
  let processing = false;
  let gotWalletToken = false;
  const runTimestamp = Math.floor(new Date().getTime() / 1000);

  const raydiumSubscriptionId = solanaConnection.onProgramAccountChange(
    RAYDIUM_LIQUIDITY_PROGRAM_ID_V4,
    async (updatedAccountInfo) => {
      if (processing) return;
      const key = updatedAccountInfo.accountId.toString();
      const poolState = LIQUIDITY_STATE_LAYOUT_V4.decode(updatedAccountInfo.accountInfo.data);
      const poolOpenTime = parseInt(poolState.poolOpenTime.toString());
      const existing = existingLiquidityPools.has(key);

      if (poolOpenTime > runTimestamp && !existing) {
        if (processing) return;
        processing = true;
        existingLiquidityPools.add(key);
        const _ = processRaydiumPool(updatedAccountInfo.accountId, poolState);
      }
    },
    process.env.COMMITMENT as Commitment,
    [
      { dataSize: LIQUIDITY_STATE_LAYOUT_V4.span },
      {
        memcmp: {
          offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('quoteMint'),
          bytes: quoteToken.mint.toBase58(),
        },
      },
      {
        memcmp: {
          offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('marketProgramId'),
          bytes: OPENBOOK_PROGRAM_ID.toBase58(),
        },
      },
      {
        memcmp: {
          offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('status'),
          bytes: bs58.encode([6, 0, 0, 0, 0, 0, 0, 0]),
        },
      },
    ],
  );

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
    process.env.COMMITMENT as Commitment,
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
      console.log(accountData);
      if (accountData.mint.toString() === quoteToken.mint.toString()) {
        logger.info('Sell confirmed');
        gotWalletToken = false;
        processing = false;
        const walletBalance = new TokenAmount(Token.WSOL, accountData.amount, true);
        logger.info(walletBalance.toString());
        return;
      }
      if (updatedAccountInfo.accountId.equals(quoteTokenAssociatedAddress)) {
        return;
      }
      if (gotWalletToken) return;
      gotWalletToken = true;
      logger.info(`Selling?`);
      let priceFetchTry = 0;
      while (price === 0) {
        price = (await getTokenPrice(watchTokenAddress)) ?? 0;
        logger.info(`Price, ${price}`);
        await new Promise((resolve) => setTimeout(resolve, 500));
        priceFetchTry += 1;
        if (priceFetchTry >= 15) {
          const _ = sellJito(
            accountData.mint,
            accountData.amount,
            existingTokenAccountsExtended,
            quoteTokenAssociatedAddress,
          );
          return;
        }
      }
      await monitorToken(watchTokenAddress, price);
      const _ = sellJito(
        accountData.mint,
        accountData.amount,
        existingTokenAccountsExtended,
        quoteTokenAssociatedAddress,
      );
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

async function monitorToken(address: string, initialPrice: number) {
  const stopLossPrecents = Number(process.env.STOP_LOSS_PERCENTS!) * -1;
  const takeProfitPercents = Number(process.env.TAKE_PROFIT_PERCENTS!);
  const timeToSellTimeout = new Date();
  timeToSellTimeout.setTime(timeToSellTimeout.getTime() + 60 * 4 * 1000);
  let timeToSellTimeoutByPriceNotChanging = new Date();
  timeToSellTimeoutByPriceNotChanging.setTime(timeToSellTimeoutByPriceNotChanging.getTime() + 150 * 1000);
  let percentageGainCurrent = 0;
  const increasePriceDelay = 4;
  let currentSuddenPriceIncrease = 0;
  const megaPriceIncrease = 18;
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
      timeToSellTimeoutByPriceNotChanging.setTime(timeToSellTimeoutByPriceNotChanging.getTime() + 120 * 1000);
      console.log(percentageGain);
    }
    if (percentageGain > megaPriceIncrease) {
      currentSuddenPriceIncrease++;
    } else currentSuddenPriceIncrease = 0;
    if (percentageGain <= stopLossPrecents) {
      logger.warn(`Selling at ${tokenPrice}$ LOSS, loss ${percentageGain}%`);
      // sendMessage(`🔴Selling ${token.symbol} at ${tokenPrice}$ LOSS, loss ${percentageGain}%🔴`);
      // await sellToken(token);
      // await closeAccount(selectedTokenAccount.pubkey);
      return;
    }
    if (percentageGain >= takeProfitPercents) {
      if (percentageGain > megaPriceIncrease) {
        if (currentSuddenPriceIncrease >= increasePriceDelay) {
          logger.info(`Selling at ${tokenPrice}$ TAKEPROFIT, increase ${percentageGain}%`);
          return;
        } else continue;
      }
      logger.info(`Selling at ${tokenPrice}$ TAKEPROFIT, increase ${percentageGain}%`);
      // sendMessage(`🟢Selling ${token.symbol} at ${tokenPrice}$ TAKEPROFIT, increase ${percentageGain}%🟢`);
      // await sellToken(token);
      // await closeAccount(selectedTokenAccount.pubkey);
      return;
    }
    if (new Date() >= timeToSellTimeout || new Date() >= timeToSellTimeoutByPriceNotChanging) {
      logger.info(`Selling at ${tokenPrice}$ TIMEOUT, change ${percentageGain}%`);
      // sendMessage(`⏰Selling ${token.symbol} at ${tokenPrice}$ TIMEOUT, change ${percentageGain}%⏰`);
      // await sellToken(token);
      // await closeAccount(selectedTokenAccount.pubkey);
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

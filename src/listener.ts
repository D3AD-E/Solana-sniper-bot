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
} from '@raydium-io/raydium-sdk';
import { getMint } from '@solana/spl-token';
import { Commitment, Connection, PublicKey } from '@solana/web3.js';
import {
  closeAccount,
  createAccount,
  getPoolKeys,
  getTokenAccounts,
  getTokenBalanceSpl,
  preformSwap,
  preformSwapJito,
} from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { MAX_REFRESH_DELAY, MIN_REFRESH_DELAY, TOKENS_FILE_NAME } from './constants';
import {
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

let existingTokenAccounts: TokenAccount[] = [];
let boughtPoolKeys: LiquidityPoolKeysV4[] = [];

const quoteToken = Token.WSOL;
let selectedTokenAccount: TokenAccount;
let swapAmount: TokenAmount;
let quoteTokenAssociatedAddress: PublicKey;
let liquidityPoolKeys: LiquidityPoolKeys[] = [];
export default async function listen(): Promise<void> {
  logger.info(`Wallet Address: ${wallet.publicKey}`);
  swapAmount = new TokenAmount(Token.WSOL, process.env.SWAP_SOL_AMOUNT, false);

  logger.info(`Swap sol amount: ${swapAmount.toFixed()} ${quoteToken.symbol}`);
  liquidityPoolKeys = await loadPoolKeys();
  logger.info(`Regenerated keys`);

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
  await new Promise((resolve) => setTimeout(resolve, 1000));
  try {
    while (true) {
      const token = await monitorDexTools();
      await monitorToken(token);
    }
  } catch (e) {
    console.log(e);
    sendMessage(`🟥🟥🟥App crashed!🟥🟥🟥`);
  }
}

async function Test() {
  // const tokenInfo = {
  //   tokenAddress: '',
  //   pairAddress: '',
  // };
  // const poolKeys = await getPoolKeysToWSOL(new PublicKey(tokenInfo.tokenAddress), tokenInfo.pairAddress);
  // selectedTokenAccount = await getSelectedAccount(tokenInfo.tokenAddress);
  // const amount = await getTokenBalanceSpl(selectedTokenAccount);
  // const txId = await preformSwapJito(
  //   tokenInfo.tokenAddress,
  //   amount,
  //   poolKeys!,
  //   selectedTokenAccount.pubkey,
  //   quoteTokenAssociatedAddress,
  //   true,
  // );
  // await preformSwapJito(
  //   tokenInfo.tokenAddress,
  //   Number(process.env.SWAP_SOL_AMOUNT),
  //   poolKeys!,
  //   selectedTokenAccount.pubkey,
  //   quoteTokenAssociatedAddress,
  // );
  // await createAccount(tokenInfo.tokenAddress, poolKeys, quoteToken);
}

async function monitorDexTools() {
  await clearTokenData();
  let isFirstRun = true;
  while (true) {
    const newTokens = await loadNewTokens();
    if (isFirstRun) {
      logger.info('First run, skipping');
      isFirstRun = false;
      continue;
    }
    for (const token of newTokens) {
      logger.info(`Got new token ${token.url} ${token.symbol}`);
      const tokenInfo = await getSwapInfo(token.url);
      if (tokenInfo === undefined) {
        logger.warn('Price too low');
        continue;
      }
      if (getProviderType(tokenInfo.exchangeString) !== ProviderType.Raydium) {
        logger.warn('Only raydium pairs are supported');
        continue;
      }
      logger.info(
        `Got new token info ${tokenInfo.tokenAddress} ${tokenInfo.pairAddress}, price ${tokenInfo.initialPrice}`,
      );
      sendMessage(
        `ℹTrying to buy a token ${token.symbol} ${tokenInfo.initialPrice}$ ${tokenInfo.tokenAddress} ${tokenInfo.pairAddress} ${token.url}`,
      );
      const poolKeys = await getPoolKeysToWSOL(new PublicKey(tokenInfo.tokenAddress), tokenInfo.pairAddress);
      await createAccount(tokenInfo.tokenAddress, poolKeys, quoteToken);

      selectedTokenAccount = await getSelectedAccount(tokenInfo.tokenAddress);

      const shouldBuyToken = await shouldBuy(tokenInfo.tokenAddress);
      if (!shouldBuyToken) {
        logger.info(`Skipping token`);
        sendMessage(`Skipping token`);
        await closeAccount(selectedTokenAccount.pubkey);
        continue;
      }

      const txId = await buyToken(tokenInfo.tokenAddress, poolKeys!);
      if (txId === undefined) {
        logger.info(`Failed to buy ${tokenInfo.tokenAddress} ${tokenInfo.pairAddress}`);
        sendMessage(
          `Failed to buy a token ${token.symbol} ${tokenInfo.initialPrice}$ ${tokenInfo.tokenAddress} ${tokenInfo.pairAddress}`,
        );
        await closeAccount(selectedTokenAccount.pubkey);
        continue;
      }
      await new Promise((resolve) => setTimeout(resolve, 200));
      const tokenPrice = await getTokenPrice(tokenInfo.tokenAddress);

      let amount = 0;
      while (amount === 0) {
        selectedTokenAccount = await getSelectedAccount(tokenInfo.tokenAddress);
        await new Promise((resolve) => setTimeout(resolve, 1000));
        amount = await getTokenBalanceSpl(selectedTokenAccount);
      }
      sendMessage(`🆗Bought ${amount} ${txId} at ${tokenPrice}`);
      console.log({
        mintAddress: tokenInfo.pairAddress,
        address: tokenInfo.tokenAddress,
        initialPrice: tokenPrice!,
        amount: amount,
        symbol: token.symbol,
      });
      boughtPoolKeys.push(poolKeys);
      return {
        mintAddress: tokenInfo.pairAddress,
        address: tokenInfo.tokenAddress,
        initialPrice: tokenPrice!,
        amount: amount,
        symbol: token.symbol,
      } as BoughtTokenData;
    }
    const randomInterval = Math.random() * (MAX_REFRESH_DELAY - MIN_REFRESH_DELAY) + MIN_REFRESH_DELAY;
    await new Promise((resolve) => setTimeout(resolve, randomInterval));
  }
}

async function getSelectedAccount(address: string) {
  let accountInfo = undefined;
  while (accountInfo === undefined) {
    existingTokenAccounts = await getTokenAccounts(
      solanaConnection,
      wallet.publicKey,
      process.env.COMMITMENT as Commitment,
    );
    const token = existingTokenAccounts.find((x) => x.accountInfo.mint.toString() === address);
    accountInfo = token?.accountInfo;
    if (accountInfo === undefined || token === undefined) {
      await new Promise((resolve) => setTimeout(resolve, 100));
      console.log('Failed to find token');
      continue;
    }
    return token;
  }
  throw 'Cannot find token';
}

async function shouldBuy(address: string) {
  console.log('ShouldBuy');
  const timeToSellTimeout = new Date();
  timeToSellTimeout.setTime(timeToSellTimeout.getTime() + 250 * 1000);
  let currentPrice = (await getTokenPrice(address)) ?? 0;

  const waitForBuysAmount = 2;
  let currentBuysAmount = 0;
  while (true) {
    const tokenPrice = await getTokenPrice(address);
    if (tokenPrice === undefined) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      continue;
    }

    const percentageGain = ((tokenPrice - currentPrice) / currentPrice) * 100;
    if (percentageGain !== 0) {
      console.log(percentageGain);
      currentPrice = tokenPrice;
      if (percentageGain > 1) {
        currentBuysAmount++;
        if (waitForBuysAmount <= currentBuysAmount) return true;
      } else {
        currentBuysAmount = 0;
      }
    }
    if (new Date() >= timeToSellTimeout) {
      return false;
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
}

async function getPoolKeysToWSOL(address: PublicKey, id: string) {
  const keys = findPoolInfoForTokensById(liquidityPoolKeys, id);
  if (keys) return keys;
  liquidityPoolKeys = await regeneratePoolKeys();
  const keysAfterRefresh = findPoolInfoForTokensById(liquidityPoolKeys, id);
  if (keysAfterRefresh) return keysAfterRefresh;

  try {
    const poolKeys = await getPoolKeys(address, new PublicKey('So11111111111111111111111111111111111111112'));
    return poolKeys;
  } catch (e) {
    const poolKeys = await getPoolKeys(new PublicKey('So11111111111111111111111111111111111111112'), address);
    return poolKeys;
  }
}

async function monitorToken(token: BoughtTokenData) {
  const stopLossPrecents = Number(process.env.STOP_LOSS_PERCENTS!) * -1;
  const takeProfitPercents = Number(process.env.TAKE_PROFIT_PERCENTS!);
  const timeToSellTimeout = new Date();
  timeToSellTimeout.setTime(timeToSellTimeout.getTime() + 60 * 30 * 1000);
  let timeToSellTimeoutByPriceNotChanging = new Date();
  timeToSellTimeoutByPriceNotChanging.setTime(timeToSellTimeoutByPriceNotChanging.getTime() + 150 * 1000);
  let percentageGainCurrent = 0;
  while (true) {
    const tokenPrice = await getTokenPrice(token.address);
    if (tokenPrice === undefined) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      continue;
    }
    const percentageGain = ((tokenPrice - token.initialPrice) / token.initialPrice) * 100;
    if (percentageGainCurrent !== percentageGain) {
      percentageGainCurrent = percentageGain;
      timeToSellTimeoutByPriceNotChanging = new Date();
      timeToSellTimeoutByPriceNotChanging.setTime(timeToSellTimeoutByPriceNotChanging.getTime() + 120 * 1000);
      console.log(percentageGain);
      if (percentageGain - percentageGainCurrent > 7) {
        continue;
      }
    }
    if (percentageGain <= stopLossPrecents) {
      logger.warn(`Selling ${token.symbol} at ${tokenPrice}$ LOSS, loss ${percentageGain}%`);
      sendMessage(`🔴Selling ${token.symbol} at ${tokenPrice}$ LOSS, loss ${percentageGain}%🔴`);
      await sellToken(token);
      await closeAccount(selectedTokenAccount.pubkey);
      return;
    }
    if (percentageGain >= takeProfitPercents) {
      logger.info(`Selling ${token.symbol} at ${tokenPrice}$ TAKEPROFIT, increase ${percentageGain}%`);
      sendMessage(`🟢Selling ${token.symbol} at ${tokenPrice}$ TAKEPROFIT, increase ${percentageGain}%🟢`);
      await sellToken(token);
      await closeAccount(selectedTokenAccount.pubkey);
      return;
    }
    if (new Date() >= timeToSellTimeout || new Date() >= timeToSellTimeoutByPriceNotChanging) {
      logger.info(`Selling ${token.symbol} at ${tokenPrice}$ TIMEOUT, change ${percentageGain}%`);
      sendMessage(`⏰Selling ${token.symbol} at ${tokenPrice}$ TIMEOUT, change ${percentageGain}%⏰`);
      await sellToken(token);
      await closeAccount(selectedTokenAccount.pubkey);
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
}

async function sellToken(token: BoughtTokenData) {
  const sellTries = 7;
  let currentTries = 0;
  const poolKeys = boughtPoolKeys.pop();

  while (sellTries > currentTries) {
    try {
      const txId = await preformSwap(
        token.address,
        token.amount,
        poolKeys!,
        selectedTokenAccount.pubkey,
        quoteTokenAssociatedAddress,
        true,
      );
      sendMessage(`💸Sold ${txId}`);
      return;
    } catch (e) {
      sendMessage(`Retrying sell`);
      currentTries++;
      console.log(e);
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  throw 'Could not sell';
}

async function buyToken(address: string, poolKeys: LiquidityPoolKeysV4) {
  const buyTries = 2;
  let currentTries = 0;

  while (buyTries > currentTries) {
    try {
      const txId = await preformSwap(
        address,
        Number(process.env.SWAP_SOL_AMOUNT),
        poolKeys!,
        selectedTokenAccount.pubkey,
        quoteTokenAssociatedAddress,
      );
      console.log(txId);
      return txId;
    } catch (e) {
      sendMessage(`Retrying buy`);

      currentTries++;
      console.log(e);
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
  }
  return undefined;
}

async function clearTokenData() {
  await writeFile(TOKENS_FILE_NAME, '');
}

export async function loadNewTokens(): Promise<TokenInfo[]> {
  try {
    if (existsSync(TOKENS_FILE_NAME)) {
      const data = JSON.parse((await readFile(TOKENS_FILE_NAME)).toString()) as TokenInfo[];
      const tokens = await getLastUpdatedTokens();
      if (tokens === undefined) {
        logger.error('Could not load tokens');
        return [];
      }
      const toret = tokens.filter((x) => !data.some((d) => d.url === x.url));
      data.push(...toret);
      await writeFile(TOKENS_FILE_NAME, JSON.stringify(data));
      return toret;
    }

    throw new Error('no file found');
  } catch (error) {
    const tokens = await getLastUpdatedTokens();
    if (tokens === undefined) {
      logger.error('Could not load tokens');
      return [];
    }
    await writeFile(TOKENS_FILE_NAME, JSON.stringify(tokens));
    return tokens;
  }
}

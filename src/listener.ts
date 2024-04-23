import {
  TokenAmount,
  Token,
  BigNumberish,
  Liquidity,
  LiquidityStateV4,
  LiquidityPoolKeysV4,
  SPL_ACCOUNT_LAYOUT,
  TOKEN_PROGRAM_ID,
  LiquidityPoolKeys,
  TokenAccount,
  PoolInfoLayout,
  SqrtPriceMath,
  LIQUIDITY_STATE_LAYOUT_V4,
  MARKET_STATE_LAYOUT_V3,
  SPL_MINT_LAYOUT,
  Market,
  Spl,
} from '@raydium-io/raydium-sdk';
import {
  createAssociatedTokenAccountIdempotentInstruction,
  createCloseAccountInstruction,
  getAccount,
  getAssociatedTokenAddressSync,
  getMint,
} from '@solana/spl-token';
import {
  Commitment,
  ComputeBudgetProgram,
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import bs58 from 'bs58';
import {
  createPoolKeys,
  getMinimalMarketV3,
  getTokenAccounts,
  MinimalMarketLayoutV3,
  OPENBOOK_PROGRAM_ID,
} from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { MAX_REFRESH_DELAY, MAX_SELL_RETRIES, MIN_REFRESH_DELAY, TOKENS_FILE_NAME } from './constants';
import {
  calcAmountOut,
  findPoolInfoForTokensById,
  findPoolInfoForTokens as findPoolKeysForTokens,
  loadPoolKeys,
} from './cryptoQueries/raydiumSwapUtils/liquidity';
import { existsSync } from 'fs';
import { readFile, writeFile } from 'fs/promises';
import { TokenInfo, getLastUpdatedTokens, getSwapInfo } from './browser/scrape';
import { sendMessage } from './telegramBot';
import { getTokenPrice } from './birdEye';
import BigNumber from 'bignumber.js';

type BoughtTokenData = {
  address: string;
  mintAddress: string;
  initialPrice: number;
  amount: number;
  symbol: string;
};

let existingTokenAccounts: TokenAccount[] = [];
let boughtPoolKeys: LiquidityPoolKeysV4[] = [];

const quoteToken = Token.WSOL;
let selectedTokenAccount: TokenAccount;
let swapAmount: TokenAmount;
let minPoolSize: TokenAmount;
let quoteTokenAssociatedAddress: PublicKey;

export default async function listen(): Promise<void> {
  logger.info(`Wallet Address: ${wallet.publicKey}`);

  swapAmount = new TokenAmount(Token.WSOL, process.env.SWAP_SOL_AMOUNT, false);
  minPoolSize = new TokenAmount(quoteToken, process.env.MIN_POOL_SIZE, false);

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
      logger.info(
        `Got new token info ${tokenInfo.links[0].id} ${tokenInfo.links[1].id}, price ${tokenInfo.initialPrice}`,
      );
      sendMessage(
        `ℹTrying to buy a token ${token.symbol} ${tokenInfo.initialPrice}$ ${tokenInfo.links[0].id} ${tokenInfo.links[1].id}`,
      );
      const poolKeys = await getPoolKeysToWSOL(new PublicKey(tokenInfo.links[0].id));

      await createAccount(tokenInfo.links[0].id, poolKeys);

      let accountInfo = undefined;
      while (accountInfo === undefined) {
        existingTokenAccounts = await getTokenAccounts(
          solanaConnection,
          wallet.publicKey,
          process.env.COMMITMENT as Commitment,
        );
        const token = existingTokenAccounts.find((x) => x.accountInfo.mint.toString() === tokenInfo.links[0].id);
        accountInfo = token?.accountInfo;
        if (accountInfo === undefined || token === undefined) {
          await new Promise((resolve) => setTimeout(resolve, 100));
          console.log('Failed to find token');
          continue;
        }
        selectedTokenAccount = token;
      }

      const shouldBuyToken = await shouldBuy(tokenInfo.links[0].id);
      if (!shouldBuyToken) {
        logger.info(`Skipping token`);
        sendMessage(`Skipping token`);
        await closeAccount(selectedTokenAccount.pubkey);
        continue;
      }

      const txId = await buyToken(tokenInfo.links[0].id, poolKeys!);
      if (txId === undefined) {
        logger.info(`Failed to buy ${tokenInfo.links[0].id} ${tokenInfo.links[1].id}`);
        sendMessage(
          `Failed to buy a token ${token.symbol} ${tokenInfo.initialPrice}$ ${tokenInfo.links[0].id} ${tokenInfo.links[1].id}`,
        );
        await closeAccount(selectedTokenAccount.pubkey);
        continue;
      }
      await new Promise((resolve) => setTimeout(resolve, 200));
      const tokenPrice = await getTokenPrice(tokenInfo.links[0].id);
      let updatedAccountInfo = undefined;
      while (updatedAccountInfo === undefined) {
        existingTokenAccounts = await getTokenAccounts(
          solanaConnection,
          wallet.publicKey,
          process.env.COMMITMENT as Commitment,
        );
        const token = existingTokenAccounts.find((x) => x.accountInfo.mint.toString() === tokenInfo.links[0].id);
        updatedAccountInfo = token?.accountInfo;
        if (accountInfo === undefined || token === undefined) {
          await new Promise((resolve) => setTimeout(resolve, 100));
          console.log('Failed to find token');
          continue;
        }
        selectedTokenAccount = token;
      }
      await new Promise((resolve) => setTimeout(resolve, 1000));
      const amount = await getTokenBalanceSpl();
      sendMessage(`🆗Bought ${amount} ${txId} at ${tokenPrice}`);
      console.log({
        mintAddress: tokenInfo.links[1].id,
        address: tokenInfo.links[0].id,
        initialPrice: tokenPrice!,
        amount: amount,
        symbol: token.symbol,
      });
      boughtPoolKeys.push(poolKeys);
      return {
        mintAddress: tokenInfo.links[1].id,
        address: tokenInfo.links[0].id,
        initialPrice: tokenPrice!,
        amount: amount,
        symbol: token.symbol,
      } as BoughtTokenData;
    }
    const randomInterval = Math.random() * (MAX_REFRESH_DELAY - MIN_REFRESH_DELAY) + MIN_REFRESH_DELAY;
    await new Promise((resolve) => setTimeout(resolve, randomInterval));
    // logger.info('Refresh');
  }
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

async function getPoolKeysToWSOL(address: PublicKey) {
  try {
    const poolKeys = await getPoolKeys(address, new PublicKey('So11111111111111111111111111111111111111112'));
    return poolKeys;
  } catch (e) {
    const poolKeys = await getPoolKeys(new PublicKey('So11111111111111111111111111111111111111112'), address);
    return poolKeys;
  }
}

async function getPoolKeys(base: PublicKey, quote: PublicKey) {
  const rsp = await fetchMarketAccounts(base, quote);
  const poolKeys = await formatAmmKeysById(rsp[0].id, solanaConnection);
  return poolKeys;
}

async function formatAmmKeysById(id: string, connection: Connection): Promise<LiquidityPoolKeysV4> {
  const account = await solanaConnection.getAccountInfo(new PublicKey(id));
  if (account === null) throw Error(' get id info error ');
  const info = LIQUIDITY_STATE_LAYOUT_V4.decode(account.data);

  const marketId = info.marketId;
  const marketAccount = await connection.getAccountInfo(marketId);
  if (marketAccount === null) throw Error(' get market info error');
  const marketInfo = MARKET_STATE_LAYOUT_V3.decode(marketAccount.data);

  const lpMint = info.lpMint;
  const lpMintAccount = await connection.getAccountInfo(lpMint);
  if (lpMintAccount === null) throw Error(' get lp mint info error');
  const lpMintInfo = SPL_MINT_LAYOUT.decode(lpMintAccount.data);

  return {
    id: new PublicKey(id),
    baseMint: info.baseMint,
    quoteMint: info.quoteMint,
    lpMint: info.lpMint,
    baseDecimals: info.baseDecimal.toNumber(),
    quoteDecimals: info.quoteDecimal.toNumber(),
    lpDecimals: lpMintInfo.decimals,
    version: 4,
    programId: account.owner,
    authority: Liquidity.getAssociatedAuthority({ programId: account.owner }).publicKey,
    openOrders: info.openOrders,
    targetOrders: info.targetOrders,
    baseVault: info.baseVault,
    quoteVault: info.quoteVault,
    withdrawQueue: info.withdrawQueue,
    lpVault: info.lpVault,
    marketVersion: 3,
    marketProgramId: info.marketProgramId,
    marketId: info.marketId,
    marketAuthority: Market.getAssociatedAuthority({ programId: info.marketProgramId, marketId: info.marketId })
      .publicKey,
    marketBaseVault: marketInfo.baseVault,
    marketQuoteVault: marketInfo.quoteVault,
    marketBids: marketInfo.bids,
    marketAsks: marketInfo.asks,
    marketEventQueue: marketInfo.eventQueue,
    lookupTableAccount: PublicKey.default,
  };
}

async function fetchMarketAccounts(base: PublicKey, quote: PublicKey) {
  const marketProgramId = new PublicKey('675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8');
  const accounts = await solanaConnection.getProgramAccounts(marketProgramId, {
    commitment: process.env.COMMITMENT as Commitment,
    filters: [
      { dataSize: LIQUIDITY_STATE_LAYOUT_V4.span },
      {
        memcmp: {
          offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('baseMint'),
          bytes: base.toBase58(),
        },
      },
      {
        memcmp: {
          offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('quoteMint'),
          bytes: quote.toBase58(),
        },
      },
    ],
  });

  return accounts.map(({ pubkey, account }) => ({
    id: pubkey.toString(),
    ...LIQUIDITY_STATE_LAYOUT_V4.decode(account.data),
  }));
}

async function getTokenBalanceSpl() {
  const amount = Number(selectedTokenAccount.accountInfo.amount);
  const mint = await getMint(solanaConnection, selectedTokenAccount.accountInfo.mint);
  const balance = amount / 10 ** mint.decimals;
  return balance;
}

async function monitorToken(token: BoughtTokenData) {
  const stopLossPrecents = Number(process.env.STOP_LOSS_PERCENTS!) * -1;
  const takeProfitPercents = Number(process.env.TAKE_PROFIT_PERCENTS!);
  const timeToSellTimeout = new Date();
  timeToSellTimeout.setTime(timeToSellTimeout.getTime() + 60 * 30 * 1000);
  const timeToSellTimeoutByPriceNotChanging = new Date();
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
      const timeToSellTimeoutByPriceNotChanging = new Date();
      timeToSellTimeoutByPriceNotChanging.setTime(timeToSellTimeoutByPriceNotChanging.getTime() + 120 * 1000);
      console.log(percentageGain);
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
      const txId = await preformSwap(token.address, token.amount, poolKeys!, 150000, true);
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
      const txId = await preformSwap(address, Number(process.env.SWAP_SOL_AMOUNT), poolKeys!, 150000);
      console.log(txId);
      return txId;
    } catch (e) {
      sendMessage(`Retrying buy`);

      currentTries++;
      console.log(e);
      await new Promise((resolve) => setTimeout(resolve, 100));
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

async function preformSwap(
  toToken: string,
  amount: number,
  poolKeys: LiquidityPoolKeys,
  maxLamports: number = 150000,
  shouldSell: boolean = false,
  slippage: number = 12,
): Promise<string | undefined> {
  const directionIn = shouldSell
    ? !(poolKeys.quoteMint.toString() == toToken)
    : poolKeys.quoteMint.toString() == toToken;

  const { minAmountOut, amountIn } = await calcAmountOut(solanaConnection, poolKeys, amount, slippage, directionIn);

  const { innerTransaction } = Liquidity.makeSwapFixedInInstruction(
    {
      poolKeys: poolKeys,
      userKeys: {
        tokenAccountIn: !directionIn ? quoteTokenAssociatedAddress : selectedTokenAccount.pubkey,
        tokenAccountOut: !directionIn ? selectedTokenAccount.pubkey : quoteTokenAssociatedAddress,
        owner: wallet.publicKey,
      },
      amountIn: amountIn.raw,
      minAmountOut: minAmountOut.raw,
    },
    poolKeys.version,
  );
  const recentBlockhashForSwap = await solanaConnection.getLatestBlockhash();

  const versionedTransaction = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: recentBlockhashForSwap.blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 421197 }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 101337 }),
      ...innerTransaction.instructions,
    ],
  }).compileToV0Message();
  const transaction = new VersionedTransaction(versionedTransaction);

  return await confirmTransaction(transaction, recentBlockhashForSwap);
}

async function createAccount(toToken: string, poolKeys: LiquidityPoolKeys): Promise<string | undefined> {
  const latestBlockhash = await solanaConnection.getLatestBlockhash();
  const ata = Spl.getAssociatedTokenAccount({
    mint: new PublicKey(toToken),
    owner: wallet.publicKey,
    programId: TOKEN_PROGRAM_ID,
  });

  const versionedTransaction = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: latestBlockhash.blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 421197 }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 101337 }),
      createAssociatedTokenAccountIdempotentInstruction(
        wallet.publicKey,
        ata,
        wallet.publicKey,
        poolKeys.baseMint,
        TOKEN_PROGRAM_ID,
      ),
    ],
  }).compileToV0Message();
  const transaction = new VersionedTransaction(versionedTransaction);
  const txId = await confirmTransaction(transaction, latestBlockhash);
  logger.info('Created account');
  return txId;
}

async function confirmTransaction(
  transaction: VersionedTransaction,
  latestBlockhash: { lastValidBlockHeight: any; blockhash: any },
) {
  transaction.sign([wallet]);
  const txid = await solanaConnection.sendTransaction(transaction, {
    preflightCommitment: process.env.COMMITMENT as Commitment,
  });
  const confirmation = await solanaConnection.confirmTransaction(
    {
      signature: txid,
      lastValidBlockHeight: latestBlockhash.lastValidBlockHeight,
      blockhash: latestBlockhash.blockhash,
    },
    process.env.COMMITMENT as Commitment,
  );
  if (!confirmation.value.err) {
    logger.info(txid);

    return txid;
  } else {
    console.log(confirmation.value.err);
    logger.error(`Error confirming tx`);
    throw 'Failed to confirm';
  }
}

async function closeAccount(tokenAddress: PublicKey): Promise<string | undefined> {
  const latestBlockhash = await solanaConnection.getLatestBlockhash();

  const versionedTransaction = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: latestBlockhash.blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 421197 }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 101337 }),
      createCloseAccountInstruction(tokenAddress, wallet.publicKey, wallet.publicKey),
    ],
  }).compileToV0Message();
  const transaction = new VersionedTransaction(versionedTransaction);

  const txId = await confirmTransaction(transaction, latestBlockhash);
  logger.info('Closed account');
  return txId;
}

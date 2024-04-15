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
} from '@raydium-io/raydium-sdk';
import {
  createAssociatedTokenAccountIdempotentInstruction,
  createCloseAccountInstruction,
  getAssociatedTokenAddressSync,
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
import { MinimalTokenAccountData, buy } from '.';
import { createPoolKeys, getMinimalMarketV3, getTokenAccounts, MinimalMarketLayoutV3 } from './cryptoQueries';
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

type BoughtTokenData = {
  address: string;
  mintAddress: string;
  initialPrice: number;
  amount: number;
  symbol: string;
};

let existingTokenAccounts: TokenAccount[] = [];

let quoteToken = Token.WSOL;
let wsolAddress: PublicKey;
let swapAmount: TokenAmount;
let minPoolSize: TokenAmount;
let allKeys: any;

let boughtTokenData: BoughtTokenData[] = [];
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

  const token = await monitorDexTools();
  await monitorToken(token);
}

async function monitorDexTools() {
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
      sendMessage(`Trying to buy a token ${token.symbol} ${tokenInfo.initialPrice}$`);
      allKeys = await loadPoolKeys();
      const poolKeys = findPoolInfoForTokensById(allKeys, tokenInfo.links[1].id);

      await preformSwap(tokenInfo.links[0].id, Number(process.env.SWAP_SOL_AMOUNT), poolKeys!);
      const amount = await getTokenAmount(tokenInfo.links[0].id, poolKeys!);
      const tokenPrice = await getTokenPrice(tokenInfo.links[0].id);
      console.log({
        mintAddress: tokenInfo.links[1].id,
        address: tokenInfo.links[0].id,
        initialPrice: tokenPrice!,
        amount: amount,
        symbol: token.symbol,
      });
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

async function getTokenAmount(tokenAddress: string, poolKeys: LiquidityPoolKeys) {
  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );
  const newTokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === tokenAddress);
  const bigAmount: BigNumberish = newTokenAccount!.accountInfo.amount as BigNumberish;
  const amount = bigAmount.divn(10 ** poolKeys!.baseDecimals).toNumber();
  return amount;
}

async function monitorToken(token: BoughtTokenData) {
  const stopLossPrecents = Number(process.env.STOP_LOSS_PERCENTS!) * -1;
  const takeProfitPercents = Number(process.env.TAKE_PROFIT_PERCENTS!);
  while (true) {
    const tokenPrice = await getTokenPrice(token.address);
    if (tokenPrice === undefined) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      continue;
    }
    const percentageGain = ((tokenPrice - token.initialPrice) / token.initialPrice) * 100;
    console.log(percentageGain);
    if (percentageGain <= stopLossPrecents) {
      logger.warn(`Selling ${token.symbol} at ${tokenPrice}$ LOSS, loss ${percentageGain}%`);
      sendMessage(`🔴Selling ${token.symbol} at ${tokenPrice}$ LOSS, loss ${percentageGain}%🔴`);
      await sellToken(token);
      return;
    }
    if (percentageGain >= takeProfitPercents) {
      logger.info(`Selling ${token.symbol} at ${tokenPrice}$ TAKEPROFIT, increase ${percentageGain}%`);
      sendMessage(`🟢Selling ${token.symbol} at ${tokenPrice}$ TAKEPROFIT, increase ${percentageGain}%🟢`);
      await sellToken(token);
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
}

async function sellToken(token: BoughtTokenData) {
  allKeys = await loadPoolKeys();
  const poolKeys = findPoolInfoForTokensById(allKeys, token.mintAddress);

  await preformSwap(token.address, token.amount, poolKeys!, 100000, 'out');
}

export async function loadNewTokens(): Promise<TokenInfo[]> {
  try {
    if (existsSync(TOKENS_FILE_NAME)) {
      const data = JSON.parse((await readFile(TOKENS_FILE_NAME)).toString()) as TokenInfo[];
      const tokens = await getLastUpdatedTokens();
      console.log(tokens);
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

async function getOwnerTokenAccounts() {
  const walletTokenAccount = await solanaConnection.getTokenAccountsByOwner(wallet.publicKey, {
    programId: TOKEN_PROGRAM_ID,
  });

  return walletTokenAccount.value.map((i) => ({
    pubkey: i.pubkey,
    programId: i.account.owner,
    accountInfo: SPL_ACCOUNT_LAYOUT.decode(i.account.data),
  }));
}

async function preformSwap(
  toToken: string,
  amount: number,
  poolKeys: LiquidityPoolKeys,
  maxLamports: number = 100000,
  fixedSide: 'in' | 'out' = 'in',
  slippage: number = 5,
): Promise<void> {
  const directionIn = poolKeys.quoteMint.toString() == toToken;
  const { minAmountOut, amountIn } = await calcAmountOut(solanaConnection, poolKeys, amount, slippage, directionIn);
  console.log(minAmountOut.raw, amountIn.raw);
  const userTokenAccounts = await getOwnerTokenAccounts();
  const swapTransaction = await Liquidity.makeSwapInstructionSimple({
    connection: solanaConnection,
    makeTxVersion: 0,
    poolKeys: {
      ...poolKeys,
    },
    userKeys: {
      tokenAccounts: userTokenAccounts,
      owner: wallet.publicKey,
    },
    amountIn: amountIn,
    amountOut: minAmountOut,
    fixedSide: fixedSide,
    config: {
      bypassAssociatedCheck: false,
    },
    computeBudgetConfig: {
      microLamports: maxLamports,
    },
  });

  const recentBlockhashForSwap = await solanaConnection.getLatestBlockhash();
  const instructions = swapTransaction.innerTransactions[0].instructions.filter(Boolean);

  const versionedTransaction = new VersionedTransaction(
    new TransactionMessage({
      payerKey: wallet.publicKey,
      recentBlockhash: recentBlockhashForSwap.blockhash,
      instructions: instructions,
    }).compileToV0Message(),
  );

  versionedTransaction.sign([wallet]);
  const txid = await solanaConnection.sendTransaction(versionedTransaction, {
    skipPreflight: true,
  });
  const confirmation = await solanaConnection.confirmTransaction(
    {
      signature: txid,
      lastValidBlockHeight: recentBlockhashForSwap.lastValidBlockHeight,
      blockhash: recentBlockhashForSwap.blockhash,
    },
    process.env.COMMITMENT as Commitment,
  );
  if (!confirmation.value.err) {
    logger.info(txid);
    if (fixedSide === 'in') sendMessage(`Bought ${txid}`);
    else sendMessage(`Sold ${txid}`);
  } else {
    logger.debug(confirmation.value.err);
    logger.info(`Error confirming tx`);
  }
}

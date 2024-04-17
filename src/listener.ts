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
import BigNumber from 'bignumber.js';

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
      sendMessage(
        `Trying to buy a token ${token.symbol} ${tokenInfo.initialPrice}$ ${tokenInfo.links[0].id} ${tokenInfo.links[1].id}`,
      );
      const poolKeys = await getPoolKeys(
        new PublicKey('So11111111111111111111111111111111111111112'),
        new PublicKey(tokenInfo.links[0].id),
      );
      await preformSwap(tokenInfo.links[0].id, Number(process.env.SWAP_SOL_AMOUNT), poolKeys!);
      const tokenPrice = await getTokenPrice(tokenInfo.links[0].id);
      await new Promise((resolve) => setTimeout(resolve, 1000));
      const amount = await getTokenBalanceSpl(tokenInfo.links[0].id);
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
    logger.info('Refresh');
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

async function getTokenBalanceSpl(tokenAccount: string) {
  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );
  const token = existingTokenAccounts.find((x) => x.accountInfo.mint.toString() === tokenAccount);
  const amount = Number(token!.accountInfo.amount);
  const mint = await getMint(solanaConnection, token!.accountInfo.mint);
  const balance = amount / 10 ** mint.decimals;
  return balance;
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
  const poolKeys = await getPoolKeys(
    new PublicKey('So11111111111111111111111111111111111111112'),
    new PublicKey(token.address),
  );
  await preformSwap(token.address, token.amount, poolKeys!, 100000, 'in', true);
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
  shouldSell: boolean = false,
  slippage: number = 5,
): Promise<void> {
  const directionIn = !shouldSell ? poolKeys.quoteMint.toString() == toToken : false;
  const { minAmountOut, amountIn } = await calcAmountOut(solanaConnection, poolKeys, amount, slippage, directionIn);

  const swapTransaction = await Liquidity.makeSwapInstructionSimple({
    connection: solanaConnection,
    makeTxVersion: 0,
    poolKeys: {
      ...poolKeys,
    },
    userKeys: {
      tokenAccounts: existingTokenAccounts,
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
    if (!shouldSell) sendMessage(`Bought ${txid}`);
    else sendMessage(`Sold ${txid}`);
  } else {
    console.log(confirmation.value.err);
    logger.info(`Error confirming tx`);
  }
}

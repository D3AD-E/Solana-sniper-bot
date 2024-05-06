import {
  GetStructureSchema,
  LIQUIDITY_STATE_LAYOUT_V4,
  LiquidityPoolKeysV4,
  MARKET_STATE_LAYOUT_V3,
  SPL_MINT_LAYOUT,
  Spl,
  Token,
} from '@raydium-io/raydium-sdk';
import {
  Commitment,
  ComputeBudgetProgram,
  Connection,
  PublicKey,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import {
  Liquidity,
  LiquidityPoolKeys,
  Market,
  TokenAccount,
  SPL_ACCOUNT_LAYOUT,
  publicKey,
  struct,
  MAINNET_PROGRAM_ID,
  LiquidityStateV4,
} from '@raydium-io/raydium-sdk';
import {
  TOKEN_PROGRAM_ID,
  createAssociatedTokenAccountIdempotentInstruction,
  createCloseAccountInstruction,
  getMint,
} from '@solana/spl-token';
import { MintLayout } from './cryptoQueries.types';
import logger from '../utils/logger';
import { solanaConnection, wallet } from '../solana';
import { calcAmountOut } from './raydiumSwapUtils/liquidity';
import { sendBundles } from '../jito/bundles';

export async function getTokenAccounts(connection: Connection, owner: PublicKey, commitment: Commitment) {
  const tokenResp = await connection.getTokenAccountsByOwner(
    owner,
    {
      programId: TOKEN_PROGRAM_ID,
    },
    commitment,
  );

  const accounts: TokenAccount[] = [];
  for (const { pubkey, account } of tokenResp.value) {
    accounts.push({
      pubkey,
      programId: account.owner,
      accountInfo: SPL_ACCOUNT_LAYOUT.decode(account.data),
    });
  }

  return accounts;
}

export async function checkMintable(connection: Connection, vault: PublicKey): Promise<boolean | undefined> {
  try {
    let { data } = (await connection.getAccountInfo(vault)) || {};
    if (!data) {
      return;
    }
    const deserialize = MintLayout.decode(data);
    return deserialize.mintAuthorityOption === 0;
  } catch (e) {
    logger.debug(e);
    logger.error(vault.toString(), `Failed to check if mint is renounced`);
  }
}

export async function createAccount(
  toToken: string,
  poolKeys: LiquidityPoolKeys,
  quoteToken: Token,
): Promise<string | undefined> {
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
        poolKeys.quoteMint.toString() === quoteToken.mint.toString() ? poolKeys.baseMint : poolKeys.quoteMint,
        TOKEN_PROGRAM_ID,
      ),
    ],
  }).compileToV0Message();
  const transaction = new VersionedTransaction(versionedTransaction);
  const txId = await confirmTransaction(transaction, latestBlockhash);
  logger.info('Created account');
  return txId;
}

export async function confirmTransaction(
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

export async function preformSwapJito(
  toToken: string,
  amount: number,
  poolKeys: LiquidityPoolKeys,
  tokenAccountAddress: PublicKey,
  quoteTokenAccountAddress: PublicKey,
  shouldSell: boolean = false,
  slippage: number = 7,
) {
  const directionIn = shouldSell
    ? !(poolKeys.quoteMint.toString() == toToken)
    : poolKeys.quoteMint.toString() == toToken;

  const { minAmountOut, amountIn } = await calcAmountOut(solanaConnection, poolKeys, amount, slippage, directionIn);
  const { innerTransaction } = Liquidity.makeSwapFixedInInstruction(
    {
      poolKeys: poolKeys,
      userKeys: {
        tokenAccountIn: shouldSell ? tokenAccountAddress : quoteTokenAccountAddress,
        tokenAccountOut: shouldSell ? quoteTokenAccountAddress : tokenAccountAddress,
        owner: wallet.publicKey,
      },
      amountIn: amountIn.raw,
      minAmountOut: minAmountOut.raw,
    },
    poolKeys.version,
  );
  const recentBlockhashForSwap = await solanaConnection.getLatestBlockhash('processed');

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

  confirmTransactionJito(transaction);
}

export async function confirmTransactionJito(transaction: VersionedTransaction) {
  transaction.sign([wallet]);
  sendBundles(wallet, transaction);
}

export async function closeAccount(tokenAddress: PublicKey): Promise<string | undefined> {
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

export async function preformSwap(
  toToken: string,
  amount: number,
  poolKeys: LiquidityPoolKeys,
  tokenAccountAddress: PublicKey,
  quoteTokenAccountAddress: PublicKey,
  shouldSell: boolean = false,
  slippage: number = 7,
): Promise<string | undefined> {
  const directionIn = shouldSell
    ? !(poolKeys.quoteMint.toString() == toToken)
    : poolKeys.quoteMint.toString() == toToken;

  const { minAmountOut, amountIn } = await calcAmountOut(solanaConnection, poolKeys, amount, slippage, directionIn);
  const { innerTransaction } = Liquidity.makeSwapFixedInInstruction(
    {
      poolKeys: poolKeys,
      userKeys: {
        tokenAccountIn: shouldSell ? tokenAccountAddress : quoteTokenAccountAddress,
        tokenAccountOut: shouldSell ? quoteTokenAccountAddress : tokenAccountAddress,
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

export async function getPoolKeys(base: PublicKey, quote: PublicKey) {
  const rsp = await fetchMarketAccounts(base, quote);
  const poolKeys = await formatAmmKeysById(rsp[0].id, solanaConnection);
  return poolKeys;
}

export async function formatAmmKeysById(id: string, connection: Connection): Promise<LiquidityPoolKeysV4> {
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

export async function fetchMarketAccounts(base: PublicKey, quote: PublicKey) {
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

export async function getTokenBalanceSpl(account: TokenAccount) {
  const amount = Number(account.accountInfo.amount);
  const mint = await getMint(solanaConnection, account.accountInfo.mint);
  const balance = amount / 10 ** mint.decimals;
  return balance;
}

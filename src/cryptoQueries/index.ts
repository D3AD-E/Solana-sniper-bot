import {
  BigNumberish,
  LIQUIDITY_STATE_LAYOUT_V4,
  LiquidityPoolKeysV4,
  MARKET_STATE_LAYOUT_V3,
  SPL_MINT_LAYOUT,
  Spl,
  Token,
  TokenAmount,
  Market as RMarket,
} from '@raydium-io/raydium-sdk';
import {
  Commitment,
  ComputeBudgetProgram,
  Connection,
  PublicKey,
  SystemProgram,
  TransactionConfirmationStatus,
  TransactionConfirmationStrategy,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import {
  Liquidity,
  LiquidityPoolKeys,
  TokenAccount,
  SPL_ACCOUNT_LAYOUT,
  LiquidityStateV4,
} from '@raydium-io/raydium-sdk';
import {
  TOKEN_PROGRAM_ID,
  createAssociatedTokenAccountIdempotentInstruction,
  createCloseAccountInstruction,
  getAssociatedTokenAddressSync,
  getMint,
} from '@solana/spl-token';
import { MinimalTokenAccountData, MintLayout } from './cryptoQueries.types';
import logger from '../utils/logger';
import { solanaConnection, wallet } from '../solana';
import { MinimalMarketLayoutV3, calcAmountOut, createPoolKeys, getMinimalMarketV3 } from './raydiumSwapUtils/liquidity';
import { sendBundles } from '../jito/bundles';
import { getRandomAccount } from '../jito/constants';
import { Market, OpenOrders } from '@project-serum/serum';
import { DEFAULT_TRANSACTION_COMMITMENT } from '../constants';
import { Block } from '../listener.types';

const tipAmount = Number(process.env.JITO_TIP!);

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

export async function confirmTransactionJito(transaction: VersionedTransaction, blockHash: string) {
  transaction.sign([wallet]);
  const bundleId = await sendBundles(wallet, transaction, blockHash);
  return bundleId;
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

export function saveTokenAccount(mint: PublicKey, accountData: MinimalMarketLayoutV3) {
  const ata = getAssociatedTokenAddressSync(mint, wallet.publicKey);
  const tokenAccount = <MinimalTokenAccountData>{
    address: ata,
    mint: mint,
    market: <MinimalMarketLayoutV3>{
      bids: accountData.bids,
      asks: accountData.asks,
      eventQueue: accountData.eventQueue,
    },
  };
  return tokenAccount;
}

export const toBuffer = (arr: Buffer | Uint8Array | Array<number>): Buffer => {
  if (Buffer.isBuffer(arr)) {
    return arr;
  } else if (arr instanceof Uint8Array) {
    return Buffer.from(arr.buffer, arr.byteOffset, arr.byteLength);
  } else {
    return Buffer.from(arr);
  }
};

export async function buy(
  accountId: PublicKey,
  accountData: LiquidityStateV4,
  quoteTokenAccountAddress: PublicKey,
  lamports: number,
  mint: PublicKey,
  block: Block,
) {
  const quoteAmount = new TokenAmount(Token.WSOL, Number(process.env.SWAP_SOL_AMOUNT), false);
  const market = await getMinimalMarketV3(solanaConnection, accountData.marketId, 'processed');
  logger.info(`Got market`);
  const tokenAccount = saveTokenAccount(mint, market);
  tokenAccount.poolKeys = createPoolKeys(accountId, accountData, tokenAccount.market!, market);

  const { innerTransaction } = Liquidity.makeSwapFixedInInstruction(
    {
      poolKeys: tokenAccount.poolKeys,
      userKeys: {
        tokenAccountIn: quoteTokenAccountAddress,
        tokenAccountOut: tokenAccount.address,
        owner: wallet.publicKey,
      },
      amountIn: quoteAmount.raw,
      minAmountOut: 0,
    },
    tokenAccount.poolKeys.version,
  );
  const messageV0 = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: block.blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: lamports }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 101337 }),
      createAssociatedTokenAccountIdempotentInstruction(wallet.publicKey, tokenAccount.address, wallet.publicKey, mint),
      ...innerTransaction.instructions,
    ],
  }).compileToV0Message();

  const transaction = new VersionedTransaction(messageV0);
  transaction.sign([wallet, ...innerTransaction.signers]);

  const signature = await solanaConnection.sendRawTransaction(transaction.serialize(), {
    skipPreflight: true,
  });
  logger.info(signature);
  return {
    signature: signature!,
    lastValidBlockHeight: block.lastValidBlockHeight,
    blockhash: block.blockhash,
    tokenAccount,
  };
}

// export async function buyJito(
//   accountId: PublicKey,
//   accountData: LiquidityStateV4,
//   existingTokenAccounts: Map<string, MinimalTokenAccountData>,
//   quoteTokenAccountAddress: PublicKey,
//   hash: string,
// ): Promise<string | undefined> {
//   const quoteAmount = new TokenAmount(Token.WSOL, Number(process.env.SWAP_SOL_AMOUNT), false);
//   const market = await Market.load(
//     solanaConnection,
//     accountData.marketId,
//     {
//       skipPreflight: true,
//       commitment: 'processed',
//     },
//     accountData.marketProgramId,
//   );
//   const tokenAccount = saveTokenAccount(accountData.baseMint, market);
//   tokenAccount.poolKeys = createPoolKeys(accountId, accountData, tokenAccount.market!, market);
//   existingTokenAccounts.set(accountData.baseMint.toString(), tokenAccount);

//   const { innerTransaction } = Liquidity.makeSwapFixedInInstruction(
//     {
//       poolKeys: tokenAccount.poolKeys,
//       userKeys: {
//         tokenAccountIn: quoteTokenAccountAddress,
//         tokenAccountOut: tokenAccount.address,
//         owner: wallet.publicKey,
//       },
//       amountIn: quoteAmount.raw,
//       minAmountOut: 0,
//     },
//     tokenAccount.poolKeys.version,
//   );
//   const tipAccount = getRandomAccount();

//   const tipInstruction = SystemProgram.transfer({
//     fromPubkey: wallet.publicKey,
//     toPubkey: tipAccount,
//     lamports: tipAmount,
//   });

//   const messageV0 = new TransactionMessage({
//     payerKey: wallet.publicKey,
//     recentBlockhash: hash,
//     instructions: [
//       // ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 421197 }),
//       ComputeBudgetProgram.setComputeUnitLimit({ units: 999900 }),
//       createAssociatedTokenAccountIdempotentInstruction(
//         wallet.publicKey,
//         tokenAccount.address,
//         wallet.publicKey,
//         accountData.baseMint,
//       ),
//       ...innerTransaction.instructions,
//       tipInstruction,
//     ],
//   }).compileToV0Message();
//   const transaction = new VersionedTransaction(messageV0);

//   return await confirmTransactionJito(transaction, hash);
// }

// export async function sellJito(
//   mint: PublicKey,
//   amount: BigNumberish,
//   existingTokenAccounts: Map<string, MinimalTokenAccountData>,
//   quoteTokenAccountAddress: PublicKey,
// ): Promise<string | undefined> {
//   const tokenAccount = existingTokenAccounts.get(mint.toString());

//   if (!tokenAccount) {
//     return undefined;
//   }

//   if (!tokenAccount.poolKeys) {
//     logger.warn('No pool keys found');
//     return undefined;
//   }

//   if (amount === 0) {
//     logger.warn(`Empty balance, can't sell`);
//     return undefined;
//   }
//   const recentBlockhashForSwap = await solanaConnection.getLatestBlockhash('confirmed');

//   const { innerTransaction } = Liquidity.makeSwapFixedInInstruction(
//     {
//       poolKeys: tokenAccount.poolKeys!,
//       userKeys: {
//         tokenAccountOut: quoteTokenAccountAddress,
//         tokenAccountIn: tokenAccount.address,
//         owner: wallet.publicKey,
//       },
//       amountIn: amount,
//       minAmountOut: 0,
//     },
//     tokenAccount.poolKeys!.version,
//   );

//   const tipAccount = getRandomAccount();

//   const tipInstruction = SystemProgram.transfer({
//     fromPubkey: wallet.publicKey,
//     toPubkey: tipAccount,
//     lamports: tipAmount,
//   });

//   const messageV0 = new TransactionMessage({
//     payerKey: wallet.publicKey,
//     recentBlockhash: recentBlockhashForSwap.blockhash,
//     instructions: [
//       // ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 421197 }),
//       ComputeBudgetProgram.setComputeUnitLimit({ units: 999900 }),
//       ...innerTransaction.instructions,
//       createCloseAccountInstruction(tokenAccount.address, wallet.publicKey, wallet.publicKey),
//       tipInstruction,
//     ],
//   }).compileToV0Message();
//   const transaction = new VersionedTransaction(messageV0);
//   return await confirmTransactionJito(transaction, recentBlockhashForSwap.blockhash);
// }
//lastValidBlock
export async function sell(
  amount: BigNumberish,
  tokenAccount: MinimalTokenAccountData,
  quoteTokenAccountAddress: PublicKey,
): Promise<TransactionConfirmationStrategy | undefined> {
  if (!tokenAccount) {
    return undefined;
  }

  if (!tokenAccount.poolKeys) {
    logger.warn('No pool keys found');
    return undefined;
  }

  if (amount === 0) {
    logger.warn(`Empty balance, can't sell`);
    return undefined;
  }
  const recentBlockhashForSwap = await solanaConnection.getLatestBlockhash({
    commitment: 'finalized',
  });

  const { innerTransaction } = Liquidity.makeSwapFixedInInstruction(
    {
      poolKeys: tokenAccount.poolKeys!,
      userKeys: {
        tokenAccountOut: quoteTokenAccountAddress,
        tokenAccountIn: tokenAccount.address,
        owner: wallet.publicKey,
      },
      amountIn: amount,
      minAmountOut: 0,
    },
    tokenAccount.poolKeys!.version,
  );

  const messageV0 = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: recentBlockhashForSwap.blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 201197 }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 101337 }),
      ...innerTransaction.instructions,
      createCloseAccountInstruction(tokenAccount.address, wallet.publicKey, wallet.publicKey),
    ],
  }).compileToV0Message();
  const transaction = new VersionedTransaction(messageV0);
  transaction.sign([wallet, ...innerTransaction.signers]);

  const signature = await solanaConnection.sendRawTransaction(transaction.serialize(), {
    skipPreflight: true,
  });
  logger.info(signature);
  return {
    signature: signature!,
    lastValidBlockHeight: recentBlockhashForSwap.lastValidBlockHeight,
    blockhash: recentBlockhashForSwap.blockhash,
  };
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
    marketAuthority: RMarket.getAssociatedAuthority({ programId: info.marketProgramId, marketId: info.marketId })
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

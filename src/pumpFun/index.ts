import {
  createAssociatedTokenAccountIdempotentInstruction,
  createAssociatedTokenAccountInstruction,
  getAccount,
  getAssociatedTokenAddress,
} from '@solana/spl-token';
import {
  Keypair,
  PublicKey,
  Commitment,
  Finality,
  Transaction,
  ComputeBudgetProgram,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import {
  PriorityFee,
  DEFAULT_COMMITMENT,
  DEFAULT_FINALITY,
  TransactionResult,
  sendTx,
  calculateWithSlippageBuy,
  GlobalAccount,
} from 'pumpdotfun-sdk';
import { Program, Provider } from '@coral-xyz/anchor';
import { IDL, PumpFun } from './IDL';
import { wallet, solanaConnection } from '../solana';
import logger from '../utils/logger';
import { Block } from '../listener.types';
const BN = require('bn.js');
export async function buyPump(
  buyer: Keypair,
  mint: PublicKey,
  buyAmountSol: bigint,
  globalAccount: GlobalAccount,
  provider: Provider,
  associatedBondingCurve: PublicKey,
  lamports: number,
  block: Block,
) {
  const buyAmount = globalAccount.getInitialBuyPrice(buyAmountSol);
  let buyTx = await getBuyInstructions(
    buyer.publicKey,
    mint,
    globalAccount.feeRecipient,
    buyAmount,
    buyAmountSol + buyAmountSol * 500n,
    provider,
    associatedBondingCurve,
  );
  // const block = await solanaConnection.getLatestBlockhash('finalized');
  const messageV0 = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: block.blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: lamports }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 71999 }),
      ...buyTx.instructions,
    ],
  }).compileToV0Message();

  const transaction = new VersionedTransaction(messageV0);
  transaction.sign([wallet]);
  logger.info('sending');

  const signature = await solanaConnection.sendRawTransaction(transaction.serialize(), {
    skipPreflight: true,
  });
  logger.info(signature);
  return {
    signature: signature!,
    lastValidBlockHeight: block.lastValidBlockHeight,
    blockhash: block.blockhash,
  };
}

export async function sellPump(
  buyer: Keypair,
  mint: PublicKey,
  sellAmount: bigint,
  globalAccount: GlobalAccount,
  provider: Provider,
  associatedBondingCurve: PublicKey,
  lamports: number,
) {
  let sellTx = await getSellInstructions(
    buyer.publicKey,
    mint,
    globalAccount.feeRecipient,
    sellAmount,
    provider,
    associatedBondingCurve,
  );
  const block = await solanaConnection.getLatestBlockhash('finalized');
  const messageV0 = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: block.blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: lamports }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 100000 }),
      ...sellTx.instructions,
    ],
  }).compileToV0Message();

  const transaction = new VersionedTransaction(messageV0);
  transaction.sign([wallet]);

  const signature = await solanaConnection.sendRawTransaction(transaction.serialize(), {
    skipPreflight: true,
  });
  logger.info(signature);
  return {
    signature: signature!,
    lastValidBlockHeight: block.lastValidBlockHeight,
    blockhash: block.blockhash,
  };
}

async function getBuyInstructions(
  buyer: PublicKey,
  mint: PublicKey,
  feeRecipient: PublicKey,
  amount: bigint,
  solAmount: bigint,
  provider: Provider,
  associatedBondingCurve: PublicKey,
) {
  const associatedUser = await getAssociatedTokenAddress(mint, buyer, false);

  let transaction = new Transaction();
  transaction.add(createAssociatedTokenAccountInstruction(buyer, associatedUser, buyer, mint));
  const program = new Program<PumpFun>(IDL as PumpFun, provider);

  transaction.add(
    await program.methods
      .buy(new BN(amount.toString()), new BN(solAmount.toString()))
      .accounts({
        feeRecipient: feeRecipient,
        mint: mint,
        associatedBondingCurve: associatedBondingCurve,
        associatedUser: associatedUser,
        user: buyer,
      })
      .transaction(),
  );

  return transaction;
}

async function getSellInstructions(
  seller: PublicKey,
  mint: PublicKey,
  feeRecipient: PublicKey,
  amount: bigint,
  provider: Provider,
  associatedBondingCurve: PublicKey,
) {
  const associatedUser = await getAssociatedTokenAddress(mint, seller, false);

  let transaction = new Transaction();
  const program = new Program<PumpFun>(IDL as PumpFun, provider);

  transaction.add(
    await program.methods
      .sell(new BN(amount.toString()), new BN('0'))
      .accounts({
        feeRecipient: feeRecipient,
        mint: mint,
        associatedBondingCurve: associatedBondingCurve,
        associatedUser: associatedUser,
        user: seller,
      })
      .transaction(),
  );

  return transaction;
}

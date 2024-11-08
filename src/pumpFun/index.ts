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
  SystemProgram,
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
import { getRandomAccount } from '../jito/constants';
import { sendBundles } from '../jito/bundles';
const BN = require('bn.js');
const tipAmount = Number(process.env.JITO_TIP!);
export async function buyPump(
  buyer: Keypair,
  mint: PublicKey,
  buyAmountSol: bigint,
  buyAmount: bigint,
  globalAccount: GlobalAccount,
  provider: Provider,
  associatedBondingCurve: PublicKey,
  block: Block,
  jitoTip: bigint,
) {
  const actualTip = Number(jitoTip.toString());
  // const raisedTip = Math.floor(actualTip * 1.01);
  let buyTx = await getBuyInstructions(
    buyer.publicKey,
    mint,
    globalAccount.feeRecipient,
    buyAmount,
    buyAmountSol + buyAmountSol / 100n, //2%
    provider,
    associatedBondingCurve,
  );
  try {
    const tipAccount = getRandomAccount();

    const tipInstruction = SystemProgram.transfer({
      fromPubkey: wallet.publicKey,
      toPubkey: tipAccount,
      lamports: actualTip,
    });
    const messageV0 = new TransactionMessage({
      payerKey: wallet.publicKey,
      recentBlockhash: block.blockhash,
      instructions: [ComputeBudgetProgram.setComputeUnitLimit({ units: 72000 }), ...buyTx.instructions, tipInstruction],
    }).compileToV0Message();

    const transaction = new VersionedTransaction(messageV0);
    transaction.sign([wallet]);
    logger.info('sending');
    sendBundles(wallet, transaction, block.blockhash);
  } catch (e) {
    console.error(e);
  }

  // const signature = await solanaConnection.sendRawTransaction(transaction.serialize(), {
  //   skipPreflight: true,
  // });
  // logger.info(signature);
  return;
}

export async function sellPump(
  buyer: Keypair,
  mint: PublicKey,
  sellAmount: bigint,
  globalAccount: GlobalAccount,
  provider: Provider,
  associatedBondingCurve: PublicKey,
  block: Block,
  jitoTip: bigint,
) {
  const actualTip = Number(jitoTip.toString());
  const raisedTip = Math.floor(actualTip * 1.01);
  let sellTx = await getSellInstructions(
    buyer.publicKey,
    mint,
    globalAccount.feeRecipient,
    sellAmount,
    provider,
    associatedBondingCurve,
  );

  logger.info('selling');

  const tipAccount = getRandomAccount();

  const tipInstruction = SystemProgram.transfer({
    fromPubkey: wallet.publicKey,
    toPubkey: tipAccount,
    lamports: actualTip,
  });
  const messageV0 = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: block.blockhash,
    instructions: [ComputeBudgetProgram.setComputeUnitLimit({ units: 72000 }), ...sellTx.instructions, tipInstruction],
  }).compileToV0Message();

  const transaction = new VersionedTransaction(messageV0);
  transaction.sign([wallet]);
  logger.info('sending');
  sendBundles(wallet, transaction, block.blockhash);

  const messageV0NoTip = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: block.blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 920010 }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 51111 }),
      ...sellTx.instructions,
    ],
  }).compileToV0Message();

  const transactionNoTip = new VersionedTransaction(messageV0NoTip);
  transactionNoTip.sign([wallet]);
  const txid = await solanaConnection.sendTransaction(transactionNoTip, {
    skipPreflight: true,
  });
  console.log(txid);
  return;
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

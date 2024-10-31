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
  globalAccount: GlobalAccount,
  provider: Provider,
  associatedBondingCurve: PublicKey,
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

  for (let i = 0; i < 5; i++) {
    const tipAccount = getRandomAccount();

    const tipInstruction = SystemProgram.transfer({
      fromPubkey: wallet.publicKey,
      toPubkey: tipAccount,
      lamports: tipAmount,
    });
    const messageV0 = new TransactionMessage({
      payerKey: wallet.publicKey,
      recentBlockhash: block.blockhash,
      instructions: [...buyTx.instructions, tipInstruction],
    }).compileToV0Message();

    const transaction = new VersionedTransaction(messageV0);
    transaction.sign([wallet]);
    logger.info('sending');
    const bundleId = await sendBundles(wallet, transaction, block.blockhash);
    console.log(bundleId);
  }

  // const signature = await solanaConnection.sendRawTransaction(transaction.serialize(), {
  //   skipPreflight: true,
  // });
  // logger.info(signature);
  return {
    signature: 'signature!',
    lastValidBlockHeight: 0, //block.lastValidBlockHeight,
    blockhash: 'block.blockhash',
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

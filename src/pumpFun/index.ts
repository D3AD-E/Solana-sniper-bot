import {
  createAssociatedTokenAccountIdempotentInstruction,
  createCloseAccountInstruction,
  getAssociatedTokenAddress,
  getAssociatedTokenAddressSync,
} from '@solana/spl-token';
import {
  Keypair,
  PublicKey,
  Transaction,
  ComputeBudgetProgram,
  TransactionMessage,
  VersionedTransaction,
  SystemProgram,
  TransactionInstruction,
} from '@solana/web3.js';
import { GlobalAccount } from 'pumpdotfun-sdk';
import { Program, Provider } from '@coral-xyz/anchor';
import { IDL, PumpFun } from './IDL';
import { wallet, solanaConnection } from '../solana';
import logger from '../utils/logger';
import { Block } from '../listener.types';
import { getRandomAccount } from '../jito/constants';
import { sendBundles } from '../jito/bundles';
import { ASTRA_ACCOUNTS, NEXT_BLOCK_ACCOUNTS, NODE_ONE_ACCOUNTS, SLOT_ACCOUNTS } from '../slotTrade/constants';
import {
  patchPublicKeyAt,
  patchBigIntLE,
  signMessageBytes,
  buildRawTx,
  findAllIndexesInString,
  getLamportsBasedOnTxType,
} from './utils';
import {
  sendTransactionAstra,
  sendTransactionNextBlock,
  sendTransactionNode,
  sendTransactionSlot,
} from '../keepAliveHttp/healthCheck';
import { TxType, TxTemplate, PositionPatch } from './pumpFun.types';
const { signMessage, findProgramAddress, associatedTokenAddress } = require('../../rust-native/index.node');
const BN = require('bn.js');

const tipAmount = BigInt(Number(process.env.JITO_TIP!));
const nonceAdvance = SystemProgram.nonceAdvance({
  noncePubkey: new PublicKey(process.env.NONCE_PUBLIC_KEY!),
  authorizedPubkey: wallet.publicKey,
});
let prebuiltTx: TxTemplate | undefined = undefined;
let prebuiltTxWithCu: TxTemplate | undefined = undefined;
const setComputeUnitLimit = 71990;

const staticSeed = Uint8Array.from([98, 111, 110, 100, 105, 110, 103, 45, 99, 117, 114, 118, 101]);

const programId = new PublicKey('6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P');
export async function prebuildTx(
  globalAccount: GlobalAccount,
  buyAmountSol: bigint,
  nonce: string,
  program: Program<PumpFun>,
  isTxWithCuPrice: boolean,
) {
  const dummyMint = new PublicKey('8A2uzH7cH41JRYq1UNE5aVhgY3cueXEszCfyaTadpump');
  const dummyCurve = new PublicKey('B6BmVVksXVrYH5G9QHA6bEb7ar2Hz95u94MS5Ad1L48A');
  const dummyOwnerVault = new PublicKey('5aUtgfVm2LFRKsEDh514PTaKcJ8jX1h1rMEBEBbfHRQ6');
  const dummyBuyAmount = 1n;
  const dummyTipAmount = 2n;
  const dummyCuPriceAmount = 3n;
  let oldNonce = '';
  let oldBonding: PublicKey | undefined = undefined;
  let dummyAssociatedUser: PublicKey | undefined = undefined;

  let buyTx = await getBuyInstructions(
    wallet.publicKey,
    dummyMint,
    globalAccount.feeRecipient,
    dummyBuyAmount,
    buyAmountSol + buyAmountSol / 100n + buyAmountSol / 200n,
    dummyCurve,
    dummyOwnerVault,
    program,
  );

  let tipAccount = SLOT_ACCOUNTS[1];

  let tipInstruction = SystemProgram.transfer({
    fromPubkey: wallet.publicKey,
    toPubkey: tipAccount,
    lamports: dummyTipAmount,
  });
  oldNonce = new PublicKey(nonce).toBuffer().toString('hex');
  let messageV0 = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: nonce,
    instructions: isTxWithCuPrice
      ? [
          nonceAdvance,
          tipInstruction,
          ComputeBudgetProgram.setComputeUnitLimit({ units: setComputeUnitLimit }),
          ComputeBudgetProgram.setComputeUnitPrice({ microLamports: dummyCuPriceAmount }),
          ...buyTx,
        ]
      : [
          nonceAdvance,
          tipInstruction,
          ComputeBudgetProgram.setComputeUnitLimit({ units: setComputeUnitLimit }),
          ...buyTx,
        ],
  }).compileToV0Message();
  dummyAssociatedUser = getAssociatedTokenAddressSync(dummyMint, wallet.publicKey, false);

  const [associatedBondingCurve] = PublicKey.findProgramAddressSync([staticSeed, dummyMint.toBuffer()], programId);
  oldBonding = associatedBondingCurve;
  let buf = messageV0.serialize();
  let patchedBuffer = Buffer.from(buf);
  let hexb = patchedBuffer.toString('hex');
  const res: TxTemplate = {
    tx: patchedBuffer,
    pos: {
      mintPos: findAllIndexesInString(hexb, dummyMint.toBuffer().toString('hex'))[0] / 2,
      curvePos: findAllIndexesInString(hexb, dummyCurve.toBuffer().toString('hex'))[0] / 2,
      ownerVaultPos: findAllIndexesInString(hexb, dummyOwnerVault.toBuffer().toString('hex'))[0] / 2,
      associatedUserPos: findAllIndexesInString(hexb, dummyAssociatedUser.toBuffer().toString('hex'))[0] / 2,
      bondingPos: findAllIndexesInString(hexb, oldBonding.toBuffer().toString('hex'))[0] / 2,
      noncePos: findAllIndexesInString(hexb, new PublicKey(nonce).toBuffer().toString('hex'))[0] / 2,
      tipAccountPos: findAllIndexesInString(hexb, tipAccount.toBuffer().toString('hex'))[0] / 2,
      amountPos: findAllIndexesInString(hexb, '0100000000000000')[0] / 2,
      tipAmountPos: findAllIndexesInString(hexb, '0200000000000000')[0] / 2,
      cuPricePos: findAllIndexesInString(hexb, '0300000000000000')[0] / 2,
    },
  };

  console.log(res.pos);
  if (isTxWithCuPrice) prebuiltTxWithCu = res;
  else prebuiltTx = res;
}

function patchTx(
  buf: Buffer,
  mint: PublicKey,
  curve: PublicKey,
  vault: PublicKey,
  user: PublicKey,
  bonding: PublicKey,
  nonce: PublicKey,
  amount: bigint,
  pos: PositionPatch,
) {
  patchPublicKeyAt(buf, pos.mintPos, mint);
  patchPublicKeyAt(buf, pos.curvePos, curve);
  patchPublicKeyAt(buf, pos.ownerVaultPos, vault);
  patchPublicKeyAt(buf, pos.associatedUserPos, user);
  patchPublicKeyAt(buf, pos.bondingPos, bonding);
  patchPublicKeyAt(buf, pos.noncePos, nonce);
  patchBigIntLE(buf, pos.amountPos, amount);
}

export async function buyPump(
  buyer: Keypair,
  mint: PublicKey,
  buyAmountSol: bigint,
  buyAmount: bigint,
  globalAccount: GlobalAccount,
  associatedBondingCurve: PublicKey,
  ownerVault: PublicKey,
  nonce: string,
  program: Program<PumpFun>,
  isMinimal: boolean,
) {
  try {
    const associatedUser = new PublicKey(associatedTokenAddress(mint.toBuffer(), buyer.publicKey.toBuffer()));
    const bonding = new PublicKey(findProgramAddress(Buffer.from(staticSeed), mint.toBuffer(), programId.toBase58()));
    patchTx(
      prebuiltTx!.tx,
      mint,
      associatedBondingCurve,
      ownerVault,
      associatedUser,
      bonding,
      new PublicKey(nonce),
      buyAmount,
      prebuiltTx!.pos,
    );
    patchTx(
      prebuiltTxWithCu!.tx,
      mint,
      associatedBondingCurve,
      ownerVault,
      associatedUser,
      bonding,
      new PublicKey(nonce),
      buyAmount,
      prebuiltTxWithCu!.pos,
    );

    if (isMinimal) {
      //cu send
      //node 1 cu
      patchPublicKeyAt(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAccountPos, NODE_ONE_ACCOUNTS[6]);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAmountPos, tipAmount * 21n);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 45300444n);
      sendTransactionNode(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 18039405n);
      sendTransactionNode(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      //slot cu
      patchPublicKeyAt(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAccountPos, SLOT_ACCOUNTS[1]);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAmountPos, tipAmount * 12n);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 15664149n);
      sendTransactionSlot(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      //astra cu
      patchPublicKeyAt(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAccountPos, ASTRA_ACCOUNTS[1]);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAmountPos, tipAmount * 12n);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 41963577n);
      sendTransactionAstra(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      //no cu send
      //node 1
      patchPublicKeyAt(prebuiltTx!.tx, prebuiltTx!.pos.tipAccountPos, NODE_ONE_ACCOUNTS[6]);
      patchBigIntLE(prebuiltTx!.tx, prebuiltTx!.pos.tipAmountPos, tipAmount * 24n);
      sendTransactionNode(signMessage(Buffer.from(prebuiltTx!.tx), Buffer.from(buyer.secretKey)));
      //slot
      patchPublicKeyAt(prebuiltTx!.tx, prebuiltTx!.pos.tipAccountPos, SLOT_ACCOUNTS[1]);
      patchBigIntLE(prebuiltTx!.tx, prebuiltTx!.pos.tipAmountPos, tipAmount * 24n);
      sendTransactionSlot(signMessage(Buffer.from(prebuiltTx!.tx), Buffer.from(buyer.secretKey)));
      // patchBigIntLE(prebuiltTx!.tx, prebuiltTx!.pos.tipAmountPos, tipAmount * 106n);
      // sendTransactionSlot(signMessage(Buffer.from(prebuiltTx!.tx), Buffer.from(buyer.secretKey)));
      //astra
      patchPublicKeyAt(prebuiltTx!.tx, prebuiltTx!.pos.tipAccountPos, ASTRA_ACCOUNTS[1]);
      patchBigIntLE(prebuiltTx!.tx, prebuiltTx!.pos.tipAmountPos, tipAmount * 31n);
      sendTransactionAstra(signMessage(Buffer.from(prebuiltTx!.tx), Buffer.from(buyer.secretKey)));
    } else {
      //cu send
      //node 1 cu
      patchPublicKeyAt(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAccountPos, NODE_ONE_ACCOUNTS[6]);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAmountPos, tipAmount * 21n);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 44300444n);
      sendTransactionNode(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 19039405n);
      sendTransactionNode(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      //slot cu
      patchPublicKeyAt(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAccountPos, SLOT_ACCOUNTS[1]);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAmountPos, tipAmount * 12n);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 16664149n);
      sendTransactionSlot(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      // //nextbl
      // patchPublicKeyAt(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAccountPos, NEXT_BLOCK_ACCOUNTS[1]);
      // patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAmountPos, tipAmount * 12n);
      // patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 44300444n);
      // sendTransactionNextBlock(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      //astra cu
      patchPublicKeyAt(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAccountPos, ASTRA_ACCOUNTS[1]);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.tipAmountPos, tipAmount * 12n);
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 41863577n);
      sendTransactionAstra(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      patchBigIntLE(prebuiltTxWithCu!.tx, prebuiltTxWithCu!.pos.cuPricePos, 42863577n);
      sendTransactionAstra(signMessage(Buffer.from(prebuiltTxWithCu!.tx), Buffer.from(buyer.secretKey)));
      //no cu send
      //node 1
      patchPublicKeyAt(prebuiltTx!.tx, prebuiltTx!.pos.tipAccountPos, NODE_ONE_ACCOUNTS[6]);
      patchBigIntLE(prebuiltTx!.tx, prebuiltTx!.pos.tipAmountPos, tipAmount * 25n);
      sendTransactionNode(signMessage(Buffer.from(prebuiltTx!.tx), Buffer.from(buyer.secretKey)));
      //slot
      patchPublicKeyAt(prebuiltTx!.tx, prebuiltTx!.pos.tipAccountPos, SLOT_ACCOUNTS[1]);
      patchBigIntLE(prebuiltTx!.tx, prebuiltTx!.pos.tipAmountPos, tipAmount * 25n);
      sendTransactionSlot(signMessage(Buffer.from(prebuiltTx!.tx), Buffer.from(buyer.secretKey)));
      // patchBigIntLE(prebuiltTx!.tx, prebuiltTx!.pos.tipAmountPos, tipAmount * 106n);
      // sendTransactionSlot(signMessage(Buffer.from(prebuiltTx!.tx), Buffer.from(buyer.secretKey)));
      //astra
      patchPublicKeyAt(prebuiltTx!.tx, prebuiltTx!.pos.tipAccountPos, ASTRA_ACCOUNTS[1]);
      patchBigIntLE(prebuiltTx!.tx, prebuiltTx!.pos.tipAmountPos, tipAmount * 32n);
      sendTransactionAstra(signMessage(Buffer.from(prebuiltTx!.tx), Buffer.from(buyer.secretKey)));
    }

    logger.info('MainBuy sent');
    const jitoTipAccount = getRandomAccount();
    let buyTx = await getBuyInstructions(
      buyer.publicKey,
      mint,
      globalAccount.feeRecipient,
      buyAmount,
      buyAmountSol + buyAmountSol / 100n + buyAmountSol / 200n,
      associatedBondingCurve,
      ownerVault,
      program,
    );
    const jitoTipInstruction = SystemProgram.transfer({
      fromPubkey: wallet.publicKey,
      toPubkey: jitoTipAccount,
      lamports: Number(tipAmount.toString()) * 16,
    });
    const jitoMessageV0 = new TransactionMessage({
      payerKey: wallet.publicKey,
      recentBlockhash: nonce,
      instructions: [
        nonceAdvance,
        ComputeBudgetProgram.setComputeUnitLimit({ units: setComputeUnitLimit }),
        ...buyTx,
        jitoTipInstruction,
      ],
    }).compileToV0Message();
    const jitoTransaction = new VersionedTransaction(jitoMessageV0);
    jitoTransaction.sign([wallet]);
    sendBundles(wallet, jitoTransaction);
  } catch (e) {
    console.error(e);
  }
  return;
}

function createTransfer(
  nonce: string,
  microLamports: number,
  tipInstruction: TransactionInstruction,
  buyTx: TransactionInstruction[],
) {
  const messageV0 = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: nonce,
    instructions:
      microLamports === 0
        ? [
            nonceAdvance,
            tipInstruction,
            ComputeBudgetProgram.setComputeUnitLimit({ units: setComputeUnitLimit }),
            ...buyTx,
          ]
        : [
            nonceAdvance,
            tipInstruction,
            ComputeBudgetProgram.setComputeUnitPrice({ microLamports: microLamports }),
            ComputeBudgetProgram.setComputeUnitLimit({ units: setComputeUnitLimit }),
            ...buyTx,
          ],
  }).compileToV0Message();
  return Buffer.from(messageV0.serialize());
}

export async function sellPump(
  buyer: Keypair,
  mint: PublicKey,
  sellAmount: bigint,
  globalAccount: GlobalAccount,
  provider: Provider,
  associatedBondingCurve: PublicKey,
  block: Block,
  ownerVault: PublicKey,
  tokenAccKey: PublicKey,
) {
  const actualTip = tipAmount;
  let sellTx = await getSellInstructions(
    buyer.publicKey,
    mint,
    globalAccount.feeRecipient,
    sellAmount,
    provider,
    associatedBondingCurve,
    ownerVault,
  );
  const closeAccount = createCloseAccountInstruction(tokenAccKey, wallet.publicKey, wallet.publicKey);
  const messageV0NoTip = new TransactionMessage({
    payerKey: wallet.publicKey,
    recentBlockhash: block.blockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 920010 }),
      ComputeBudgetProgram.setComputeUnitLimit({ units: 51111 }),
      ...sellTx.instructions,
      closeAccount,
    ],
  }).compileToV0Message();

  const transactionNoTip = new VersionedTransaction(messageV0NoTip);
  transactionNoTip.sign([wallet]);
  const txid = await solanaConnection.sendTransaction(transactionNoTip, {
    skipPreflight: true,
  });
  console.log(txid);

  for (let i = 0; i < 2; i += 1) {
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
      instructions: [
        ComputeBudgetProgram.setComputeUnitLimit({ units: 72000 }),
        ...sellTx.instructions,
        tipInstruction,
      ],
    }).compileToV0Message();

    const transaction = new VersionedTransaction(messageV0);
    transaction.sign([wallet]);
    logger.info('sending');
    await sendBundles(wallet, transaction);
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }

  return;
}

async function getBuyInstructions(
  buyer: PublicKey,
  mint: PublicKey,
  feeRecipient: PublicKey,
  amount: bigint,
  solAmount: bigint,
  associatedBondingCurve: PublicKey,
  creatorVault: PublicKey,
  program: Program<PumpFun>,
) {
  const associatedUser = getAssociatedTokenAddressSync(mint, buyer, false);
  console.log(associatedUser);
  const create = createAssociatedTokenAccountIdempotentInstruction(buyer, associatedUser, buyer, mint);
  const ix = await program.methods
    .buy(new BN(amount.toString()), new BN(solAmount.toString()))
    .accounts({
      mint,
      // @ts-ignore
      creatorVault,
      associatedBondingCurve,
      user: buyer,
      feeRecipient,
      associatedUser,
    })
    .instruction();

  return [create, ix];
}

async function getSellInstructions(
  seller: PublicKey,
  mint: PublicKey,
  feeRecipient: PublicKey,
  amount: bigint,
  provider: Provider,
  associatedBondingCurve: PublicKey,
  creatorVault: PublicKey,
) {
  const associatedUser = await getAssociatedTokenAddress(mint, seller, false);

  let transaction = new Transaction();
  const program = new Program<PumpFun>(IDL as PumpFun, provider);
  // @ts-ignore
  transaction.add(
    await program.methods
      .sell(new BN(amount.toString()), new BN('0'))
      .accounts({
        mint: mint,
        creatorVault: creatorVault,
        associatedBondingCurve: associatedBondingCurve,
        user: seller,
        feeRecipient: feeRecipient,
        associatedUser: associatedUser,
      } as any)
      .transaction(),
  );

  return transaction;
}

import { Keypair, PublicKey, VersionedTransaction } from '@solana/web3.js';
import { SearcherClient } from 'jito-ts/dist/sdk/block-engine/searcher';
import { solanaConnection } from '../solana';
import { Bundle } from 'jito-ts/dist/sdk/block-engine/types';
import { isError } from 'jito-ts/dist/sdk/block-engine/utils';
import { JitoClient } from './searcher';

export const sendBundles = async (wallet: Keypair, transactions: VersionedTransaction) => {
  const client = await JitoClient.getInstance();
  const _tipAccount = (await client.getTipAccounts())[0];
  console.log('tip account:', _tipAccount);
  const tipAccount = new PublicKey(_tipAccount);

  let isLeaderSlot = false;
  while (!isLeaderSlot) {
    let next_leader = await client.getNextScheduledLeader();
    let num_slots = next_leader.nextLeaderSlot - next_leader.currentSlot;
    isLeaderSlot = num_slots <= 10;
    console.log(`next jito leader slot in ${num_slots} slots`);
    await new Promise((r) => setTimeout(r, 500));
  }

  let blockHash = await solanaConnection.getLatestBlockhash('processed');
  const b = new Bundle([transactions], Number(process.env.BUNDLE_TRANSACTION_LIMIT));

  console.log(blockHash.blockhash);

  let bundles = [b];

  if (isError(b)) {
    throw b;
  }

  const maybeBundle = b.addTipTx(wallet, 30_000, tipAccount, blockHash.blockhash);

  if (isError(maybeBundle)) {
    throw maybeBundle;
  }

  const resp = await client.sendBundle(maybeBundle);
  console.log('resp:', resp);
  client.onBundleResult(
    async (bundleResult) => {
      if (resp === bundleResult.bundleId) {
        console.log('result:', bundleResult);
        return bundleResult.bundleId;
      }
    },
    (e) => {
      throw e;
    },
  );
};

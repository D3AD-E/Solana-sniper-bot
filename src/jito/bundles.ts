import { Keypair, PublicKey, VersionedTransaction } from '@solana/web3.js';
import { SearcherClient } from 'jito-ts/dist/sdk/block-engine/searcher';
import { solanaConnection } from '../solana';
import { Bundle } from 'jito-ts/dist/sdk/block-engine/types';
import { isError } from 'jito-ts/dist/sdk/block-engine/utils';
import { JitoClient } from './searcher';
import { getRandomAccount } from './constants';
import logger from '../utils/logger';

export const sendBundles = async (
  wallet: Keypair,
  transactions: VersionedTransaction,
  blockHash: string,
  shoutWaitForLeader: boolean,
) => {
  const client = await JitoClient.getInstance();
  const tipAccount = getRandomAccount();
  const b = new Bundle([transactions], Number(process.env.BUNDLE_TRANSACTION_LIMIT));
  // logger.info(shoutWaitForLeader.toString());
  // if (shoutWaitForLeader) {
  //   let nextLeader = await client.getNextScheduledLeader();
  //   console.log(nextLeader);
  //   let numSlots = nextLeader.nextLeaderSlot - nextLeader.currentSlot;
  //   console.log('Slots', numSlots);
  //   logger.info(numSlots.toString());
  //   if (numSlots > 10) return undefined;
  // }

  // if (isError(b)) {
  //   throw b;
  // }

  const maybeBundle = b.addTipTx(wallet, 500_000, tipAccount, blockHash);

  if (isError(maybeBundle)) {
    throw maybeBundle;
  }

  const resp = await client.sendBundle(maybeBundle);
  logger.info('resp: ' + resp);
  return resp;
};

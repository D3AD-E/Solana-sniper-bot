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
  retry: any,
) => {
  const client = await JitoClient.getInstance();
  const tipAccount = getRandomAccount();
  const b = new Bundle([transactions], Number(process.env.BUNDLE_TRANSACTION_LIMIT));

  if (isError(b)) {
    throw b;
  }

  const maybeBundle = b.addTipTx(wallet, 30_000, tipAccount, blockHash);

  if (isError(maybeBundle)) {
    throw maybeBundle;
  }

  const resp = await client.sendBundle(maybeBundle);
  console.log('resp:', resp);
  client.onBundleResult(
    async (bundleResult) => {
      if (resp === bundleResult.bundleId) {
        console.log('result:', bundleResult);
        logger.info('Res');
        return bundleResult.bundleId;
      }
    },
    (e) => {
      // logger.warn('Error');
      // if (retry) retry();
      throw e;
    },
  );
};

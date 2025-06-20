import { Keypair, PublicKey, VersionedTransaction } from '@solana/web3.js';
import { SearcherClient } from 'jito-ts/dist/sdk/block-engine/searcher';
import { solanaConnection } from '../solana';
import { Bundle } from 'jito-ts/dist/sdk/block-engine/types';
import { isError } from 'jito-ts/dist/sdk/block-engine/utils';
import { JitoClient } from './searcher';
import { getRandomAccount } from './constants';
import logger from '../utils/logger';

export const sendBundles = async (wallet: Keypair, transactions: VersionedTransaction) => {
  const client = await JitoClient.getInstance();
  const b = new Bundle([transactions], 1);

  // if (isError(b)) {
  //   throw b;
  // }

  // const maybeBundle = b.addTipTx(wallet, 100_000, tipAccount, blockHash);
  client.sendBundle(b).catch((e) => console.error(e));
  // if (isError(b)) {
  //   throw b;
  // }
  // try {
  //   const resp = await client.sendBundle(b);
  //   logger.info('resp: ' + resp);
  //   return resp;
  // } catch (e) {
  //   console.log(e);
  //   return 'Failed';
  // }
};

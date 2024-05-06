import { Keypair } from '@solana/web3.js';
import { SearcherClient, searcherClient } from 'jito-ts/dist/sdk/block-engine/searcher';
import { readFile, writeFile } from 'fs/promises';
import { KEYPAIR_FILE_PATH } from './constants';

export class JitoClient {
  private static instance: SearcherClient;

  private constructor() {}

  static async getInstance() {
    if (this.instance) {
      return this.instance;
    }

    const decodedKey = new Uint8Array(JSON.parse((await readFile(KEYPAIR_FILE_PATH)).toString()) as number[]);
    const keypair = Keypair.fromSecretKey(decodedKey);
    const c = searcherClient(process.env.JITO_URL!, keypair);
    this.instance = c;
    return this.instance;
  }
}

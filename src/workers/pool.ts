import { PublicKey } from '@solana/web3.js';
import { ParentMessage, WorkerAction, WorkerMessage, WorkerResult } from './worker.types';
import { RawAccount } from '@solana/spl-token';
import { MinimalTokenAccountData } from '../cryptoQueries/cryptoQueries.types';
import { Worker } from 'worker_threads';

export class WorkerPool {
  private numWorkers: number;
  private workers: Worker[];
  private freeWorkers: Worker[];
  private takenWorkers: Map<string, Worker> = new Map<string, Worker>();
  private quoteTokenAssociatedAddress: PublicKey;

  constructor(numWorkers: number, quoteTokenAssociatedAddress: PublicKey) {
    this.numWorkers = numWorkers;
    this.workers = [];
    this.freeWorkers = [];
    this.quoteTokenAssociatedAddress = quoteTokenAssociatedAddress;
    this.createWorkers();
  }

  private createWorkers() {
    for (let i = 0; i < this.numWorkers; i++) {
      const worker = new Worker('./src/workers/worker.ts', {
        execArgv: ['--require', 'ts-node/register'],
        workerData: process.env,
      });
      worker?.on('message', (message: ParentMessage) => {
        if (message.result === WorkerResult.SellSuccess) {
          this.freeWorker(message.data.token);
        }
      });
      worker?.on('error', (message: any) => {
        console.log(message);
      });
      const setupMessage: WorkerMessage = {
        action: WorkerAction.Setup,
        data: {
          quoteTokenAssociatedAddress: this.quoteTokenAssociatedAddress,
        },
      };
      worker.postMessage(setupMessage);
      this.workers.push(worker);
      this.freeWorkers.push(worker);
    }
  }

  public areThereFreeWorkers = () => this.freeWorkers.length > 0;

  public gotToken(token: string, lastRequest: any) {
    if (this.freeWorkers.length > 0) {
      const worker = this.freeWorkers.pop()!;
      this.takenWorkers.set(token, worker);
      const tokenGotMessage: WorkerMessage = {
        action: WorkerAction.GetToken,
        data: {
          token,
          lastRequest,
        },
      };
      worker.postMessage(tokenGotMessage);
    } else throw 'No free workers';
  }

  public doesTokenExist(token: string) {
    return this.takenWorkers.has(token);
  }

  public freeWorker(token: string) {
    if (!this.takenWorkers.has(token)) return;
    const worker = this.takenWorkers.get(token);
    this.freeWorkers.push(worker!);
    this.takenWorkers.delete(token);
  }

  public forceSell(token: string, accountData: RawAccount) {
    if (!this.takenWorkers.has(token)) return;
    const worker = this.takenWorkers.get(token);
    const forceSellMessage: WorkerMessage = {
      action: WorkerAction.ForceSell,
      data: {
        accountData,
      },
    };
    worker!.postMessage(forceSellMessage);
  }

  public gotWalletToken(token: string, timeToSellTimeoutGeyser: Date, foundTokenData: RawAccount) {
    const worker = this.takenWorkers.get(token);
    const tokenGotMessage: WorkerMessage = {
      action: WorkerAction.GotWalletToken,
      data: {
        timeToSellTimeoutGeyser,
        foundTokenData,
      },
    };
    worker!.postMessage(tokenGotMessage);
  }
  public addTokenAccount(token: string, tokenAccount: MinimalTokenAccountData) {
    const worker = this.takenWorkers.get(token);
    const tokenGotMessage: WorkerMessage = {
      action: WorkerAction.AddTokenAccount,
      data: {
        tokenAccount,
      },
    };
    worker!.postMessage(tokenGotMessage);
  }
}

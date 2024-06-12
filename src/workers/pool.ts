import { PublicKey } from '@solana/web3.js';
import { WorkerAction, WorkerMessage } from './worker.types';

class WorkerPool {
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
      const worker = new Worker('./worker.ts');
      worker.onmessage = (event) => {
        console.log(event);
      };
      worker.onerror = (event) => {
        console.log(event);
      };
      worker.onmessageerror = (event) => {
        console.log(event);
      };
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

  private runTask(worker: Worker, task: Task, resolve: (value: unknown) => void, reject: (reason?: any) => void) {
    worker.onmessage = (event) => {
      //from worker
    };
    worker.postMessage(task); //to worker

    return new Promise((resolve, reject) => {
      if (this.freeWorkers.length > 0) {
        const worker = this.freeWorkers.pop()!;
        this.runTask(worker, task, resolve, reject);
      }
    });
  }
}

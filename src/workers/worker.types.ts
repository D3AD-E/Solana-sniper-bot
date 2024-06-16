import { RawAccount } from '@solana/spl-token';
import { MinimalTokenAccountData } from '../cryptoQueries/cryptoQueries.types';
import { PublicKey } from '@solana/web3.js';

export interface Task {
  action: string;
  token: string;
}

export interface TaskQueueItem {
  task: Task;
  resolve: (value: unknown) => void;
  reject: (reason?: any) => void;
}

export interface WorkerMessage {
  action: WorkerAction;
  data?: {
    tokenAccount?: MinimalTokenAccountData;
    foundTokenData?: RawAccount;
    accountData?: RawAccount;
    token?: string;
    lastRequest?: any;
    quoteTokenAssociatedAddress?: PublicKey;
  };
}

export interface ParentMessage {
  result: WorkerResult;
  data?: any;
}

export enum WorkerAction {
  Setup,
  GetToken,
  GotWalletToken,
  ForceSell,
  AddTokenAccount,
  Clear,
}

export enum WorkerResult {
  SellSuccess,
}

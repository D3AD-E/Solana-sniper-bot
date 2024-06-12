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
  data?: any;
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
}

export enum WorkerResult {
  SellSuccess,
}

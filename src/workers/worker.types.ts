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
  data: any;
}

export enum WorkerAction {
  Setup,
}

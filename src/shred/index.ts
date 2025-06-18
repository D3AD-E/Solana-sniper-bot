import Client, {
  SubscribeTransactionsResponse,
  SubscribeUpdateTransaction,
  type SubscribeTransactionsRequest,
} from '@shreder_xyz/grpc-client';
import { ClientDuplexStream } from '@grpc/grpc-js';

type OnTransactionCallback = (data: SubscribeUpdateTransaction) => void;

export class ShrederClient {
  private client: Client;
  private stream?: ClientDuplexStream<SubscribeTransactionsRequest, SubscribeTransactionsResponse>;
  private onTransactionCallback?: OnTransactionCallback;

  constructor(url: string) {
    this.client = new Client(url, {});
  }

  public onTransaction(onTransactionCallback: OnTransactionCallback) {
    this.onTransactionCallback = onTransactionCallback;
  }

  public async start() {
    this.stream = await this.client.subscribe();
    const request = this.createRequest();
    await this.sendSubscribeRequest(this.stream!, request);
    this.stream!.on('data', (data: SubscribeTransactionsResponse) => {
      if (this.onTransactionCallback && data.transaction) {
        this.onTransactionCallback(data.transaction);
      }
    });
    return this.handleStreamEvents(this.stream!).catch((error) => console.log(error));
  }

  public stop() {
    this.stream?.removeAllListeners();
    this.stream?.end();
  }

  private createRequest(): SubscribeTransactionsRequest {
    return {
      transactions: {
        pumpfun: {
          accountInclude: [],
          accountExclude: [],
          accountRequired: [
            '6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P',
            'metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s',
            'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA',
            '11111111111111111111111111111111',
          ],
        },
      },
    };
  }

  private async sendSubscribeRequest(
    stream: ClientDuplexStream<SubscribeTransactionsRequest, SubscribeTransactionsResponse>,
    request: SubscribeTransactionsRequest,
  ): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const status = stream.write(request, (err: Error | null) => {
        if (err) {
          reject(err);
        } else {
          resolve();
        }
      });
    });
  }

  private async handleStreamEvents(
    stream: ClientDuplexStream<SubscribeTransactionsRequest, SubscribeTransactionsResponse>,
  ): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      stream.on('error', (error: Error) => {
        console.error('Stream error:', error);
        reject(error);
        stream.end();
      });
      stream.on('end', () => {
        console.log('Stream ended');
        resolve();
      });
      stream.on('close', () => {
        console.log('Stream closed');
        resolve();
      });
    });
  }
}

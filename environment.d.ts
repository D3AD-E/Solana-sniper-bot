import { Commitment, Connection, PublicKey } from '@solana/web3.js';

declare global {
  namespace NodeJS {
    interface ProcessEnv {
      RPC_ENDPOINT: string;
      WEBSOCKET_ENDPOINT: string;
      GEYSER_ENDPOINT: string;
      WALLET_PRIVATE_KEY: string;
      BIRDEYE_API_KEY: string;
      JITO_URL: string;
      BUNDLE_TRANSACTION_LIMIT: number;
      BOT_TOKEN: string;
      COMMITMENT: Commitment;
      TAKE_PROFIT_PERCENTS: number;
      STOP_LOSS_PERCENTS: number;
      BLOCKS_MAX_LIMIT: number;
      BLOCKS_MIN_LIMIT: number;
      SELL_TIMEOUT_SEC: number;
      JITO_TIP: number;
      SWAP_SOL_AMOUNT: number;
      CHAT_ID: number;
      MIN_POOL_SIZE: number;
    }
  }
}
export {};

import { Commitment, Connection, PublicKey } from '@solana/web3.js';

declare global {
  namespace NodeJS {
    interface ProcessEnv {
      RPC_ENDPOINT: string;
      RPC_SLOW_ENDPOINT: string;
      RPC_SLOW_WEBSOCKET_ENDPOINT: string;
      WEBSOCKET_ENDPOINT: string;
      WALLET_PRIVATE_KEY: string;
      BIRDEYE_API_KEY: string;
      JITO_URL: string;
      BOT_TOKEN: string;
      COMMITMENT: Commitment;
      JITO_TIP: number;
      SWAP_SOL_AMOUNT: number;
      CHAT_ID: number;
      SECOND_CONNECTION_KEY: string;
      NEXTBLOCK_CONNECTION_KEY: string;
      NODE_ONE_ENDPOINT: string;
      NODE_ONE_KEY: string;
      SECOND_WALLET: string;
      NONCE_PUBLIC_KEY: string;
      ASTRA_KEY: string;
      NODE_REGION: string;
    }
  }
}
export {};

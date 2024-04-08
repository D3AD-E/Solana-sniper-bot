declare global {
  namespace NodeJS {
    interface ProcessEnv {
      RPC_ENDPOINT: string;
      WEBSOCKET_ENDPOINT: string;
      WALLET_PRIVATE_KEY: string;
    }
  }
}
export {};

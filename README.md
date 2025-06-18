# Solana Sniper

This Node.js TypeScript application facilitates the sniping new pairs on Raydium for solana.

**!!! I AM NOT RESPONSIBLE FOR RISKS AND FUNDS LOSS WHILE USING THIS TOOL !!!**

By default omits all pars that can be frozen and ones created via ...pump websites

## Prerequisites

Before running this application, ensure you have the following installed:

- Node.js (version 12.x or higher)
- npm (Node Package Manager)
- TypeScript
- Solana wallet
- RPC endpoint
- Geyser endpoint
- Telegram bot
- Some amount of WSOL on wallet
- (Optional) Jito access
- (Optional) Multi core processor to allow the worker threads to perform fine

Set the environment variables in a `.env` file:

```nodejs
RPC_ENDPOINT: string; - url to rpc endpoint
WEBSOCKET_ENDPOINT: string; - url to websocket endpoint
GEYSER_ENDPOINT: string; - url to geyser endpoint
WALLET_PRIVATE_KEY: string; - key of your solana wallet
JITO_URL: string; - jito access url
BUNDLE_TRANSACTION_LIMIT: number; - used only in jito
BOT_TOKEN: string; - tg bot token
COMMITMENT: Commitment; - processed is preferred
TAKE_PROFIT_PERCENTS: number; - take profit for sell
STOP_LOSS_PERCENTS: number; - stop loss for sell
SELL_TIMEOUT_SEC: number; - timeout in secods when to sell
JITO_TIP: number; - jito tip amount
SWAP_SOL_AMOUNT: number; - amount of sol to swap
CHAT_ID: number; - chat id for tg bot
MIN_POOL_SIZE: number; - min pool size to snipe
ENABLE_PROTECTION: string; - will try to snipe as fast as possible but might fail a lot
WORKER_AMOUNT: number; - amount of tokens that can be processed at once
```
For rpc you can use free ones listed [here](https://solana.com/rpc).

## Usage
- ```npm run start``` Runs the sniper and listens for new radium pairs


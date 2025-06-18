# Solana Sniper

This Node.js TypeScript application facilitates the sniping new pairs on Pump.fun in 0 slot timings. Txs patched and sent to all providers in 5ms window.

**!!! I AM NOT RESPONSIBLE FOR RISKS AND FUNDS LOSS WHILE USING THIS TOOL !!!**

Integration with 0slot, nextblock ,astra and node1 providers.
Supports multiple region deployments (one node on the X region and replicas on Y regions)
Required shred access for fast transactions
You need to run your own node as well as redis (redis can be in docker)

PS. History purged bc I needed to push a lot of unredable commit messages to support running in multiple regions

Set the environment variables in a `.env` file:

```nodejs
# 🧾 WALLET CONFIGURATION
WALLET_PRIVATE_KEY=                                      # Private key of the main wallet (Base58 or raw format)
SECOND_WALLET=                                            # Secondary wallet public key

# 🔌 RPC CONNECTION ENDPOINTS
RPC_ENDPOINT=http://127.0.0.1:8899                   # Primary RPC endpoint (local node)
RPC_SLOW_ENDPOINT=https://mainnet.helius-rpc.com/?api-key=...  # Fallback slow RPC endpoint
RPC_SLOW_WEBSOCKET_ENDPOINT=wss://mainnet.helius-rpc.com/?api-key=...  # WebSocket version of the slow RPC
WEBSOCKET_ENDPOINT=ws://127.0.0.1:8900               # WebSocket connection to local node

# 📶 CONNECTION & PERFORMANCE
COMMITMENT=processed                                 # Commitment level: processed | confirmed | finalized
JITO_URL=ny.mainnet.block-engine.jito.wtf            # Jito block engine endpoint (used for MEV/send priority tx)
JITO_TIP=500000                                       # Amount of tip (in lamports) to include with Jito transactions

# 🤖 TELEGRAM BOT CONFIGURATION
BOT_TOKEN=                                            # Telegram bot token
CHAT_ID=                                              # Telegram chat ID for notifications

# 🔐 API KEYS & SYSTEM KEYS
SLOT_CONNECTION_KEY=                                 # Key for 0slot
NEXTBLOCK_KEY=                                       # API key for NextBlock
NODE_ONE_KEY=                                        # Key for NodeOne
NONCE_PUBLIC_KEY=                                    # Public key for durable nonce (if used)
ASTRA_KEY=                                           # Optional key for Astra 
# 🌐 CLOUD OR REGION SETTINGS

NODE_REGION=fra                                      # Node region code (e.g., fra = Frankfurt)

```



## 📦 Usage

Run the following commands from the root of the project:

```
npm run start
```
Runs the sniper and listens for new PumpFun pairs in full mode.

```
npm run start:minimal
```
Runs a lightweight version of the sniper (region copy without proper node)

```
npm run analyze
```
Analyzes recent trades or token launches for insights or patterns. Adds token wallets to whitelist to support sniping

```
npm run prod
```
Builds the project and starts it with high-memory and performance flags, suitable for production deployment.

```
npm run build
```
Compiles the TypeScript source and copies native modules and generated files into the `dist/` directory.

```
npm run build:napi
```
Compiles the native Rust module using N-API in release mode.



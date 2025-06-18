import { Commitment, Connection, Keypair } from '@solana/web3.js';
import bs58 from 'bs58';
import dotenv from 'dotenv';

dotenv.config();

export const solanaConnection = new Connection(process.env.RPC_ENDPOINT as string, {
  wsEndpoint: process.env.WEBSOCKET_ENDPOINT as string,
  commitment: process.env.COMMITMENT as Commitment,
});

export const solanaSlowConnection = solanaConnection;
// new Connection(process.env.RPC_SLOW_ENDPOINT as string, {
//   wsEndpoint: process.env.RPC_SLOW_WEBSOCKET_ENDPOINT as string,
//   commitment: 'finalized' as Commitment,
// });

export const wallet = Keypair.fromSecretKey(bs58.decode(process.env.WALLET_PRIVATE_KEY as string));

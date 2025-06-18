import { Wallet, AnchorProvider } from '@coral-xyz/anchor';
import { Commitment } from '@solana/web3.js';
import { wallet, solanaConnection, solanaSlowConnection } from '../solana';

export const getProvider = (shouldUseSlow: boolean = false) => {
  const walletAnchor = new Wallet(wallet);
  const provider = new AnchorProvider(shouldUseSlow ? solanaSlowConnection : solanaConnection, walletAnchor, {
    commitment: process.env.COMMITMENT as Commitment,
  });
  return provider;
};

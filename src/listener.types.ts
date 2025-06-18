import { PublicKey } from '@solana/web3.js';
export type BoughtTokenData = {
  address: string;
  mintAddress: string;
  initialPrice: number;
  amount: number;
  symbol: string;
};

export type BundlePacket = {
  bundleId: string;
  failAction: any;
};

export type Block = {
  blockhash: string;
  lastValidBlockHeight: number;
};

export type CurveMint = {
  mint: string;
  curve: PublicKey;
  otherPersonBuyAmount: bigint;
  otherPersonAddress: string;
  ownerVault: PublicKey;
};

export type BuyTestData = {
  mint: string;
  boughtAt: Date;
  wasSeen?: boolean;
};

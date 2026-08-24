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

/**
 * A pump.fun launch as delivered by the shredstream proxy: already parsed, raw 32 byte
 * keys, nothing to deserialize on this side.
 */
export type PumpLaunch = {
  slot: number;
  mint: Buffer;
  bondingCurve: Buffer;
  associatedBondingCurve: Buffer;
  creator: Buffer;
  user: Buffer;
  tokenProgram: Buffer;
  devBuyLamports: bigint;
  isV2: boolean;
};

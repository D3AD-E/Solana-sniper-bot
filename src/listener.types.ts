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

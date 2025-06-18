export type PumpFunTradeEvent = {
  creator: string;
  accValue: bigint;
  initialBuyAt?: number;
  createdAt: number;
  initialBuyAmount?: bigint;
  shouldBuy?: boolean;
  otherSnipersList: string[];
  watchBought?: boolean;
  walletBalance?: number;
};

export type PumpBoughtEvent = {
  diff: number;
  mint: string;
  creator: string;
  createdAt: number;
  initialBuyAmount: number;
};

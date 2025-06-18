export class PumpFunEstimator {
  VIRTUAL_SOL_RESERVES: number;
  VIRTUAL_TOKEN_RESERVES: number;
  SELL_FEE: number;
  BUY_FEE: number;
  constructor() {
    // Pump.fun bonding curve constants
    this.VIRTUAL_SOL_RESERVES = 30;
    this.VIRTUAL_TOKEN_RESERVES = 1073000000; // ~1.073B tokens
    this.SELL_FEE = 0.0; // No sell fee on pump.fun
    this.BUY_FEE = 0.01; // 1% fee on buys (taken from input)
  }

  // Calculate current price based on SOL in curve
  getCurrentPrice(solInCurve: number) {
    return (this.VIRTUAL_SOL_RESERVES + solInCurve) / this.VIRTUAL_TOKEN_RESERVES;
  }

  // Calculate tokens received for SOL buy (accounting for buy fee)
  calculateBuyTokens(solAmount: number, currentSolInCurve: number) {
    const solAfterFee = solAmount * (1 - this.BUY_FEE);
    const startPrice = this.getCurrentPrice(currentSolInCurve);
    const endPrice = this.getCurrentPrice(currentSolInCurve + solAfterFee);

    // Use average price for linear approximation
    const avgPrice = (startPrice + endPrice) / 2;
    return solAfterFee / avgPrice;
  }

  // Calculate SOL received for token sell (no sell fee on pump.fun)
  calculateSellProceeds(tokenAmount: number, currentSolInCurve: number) {
    const currentPrice = this.getCurrentPrice(currentSolInCurve);
    return tokenAmount * currentPrice; // No sell fee
  }

  // Main estimation function
  estimateSellProceeds(scenario: any) {
    const { creatorBuy, yourBuy, otherBuy } = scenario;
    // Track curve progression
    let currentSolInCurve = 0;

    // Step 1: Creator buys 5 SOL
    const creatorSolAfterFee = creatorBuy * (1 - this.BUY_FEE);
    const creatorTokens = this.calculateBuyTokens(creatorBuy, currentSolInCurve);
    currentSolInCurve += creatorSolAfterFee;

    const yourSolAfterFee = yourBuy * (1 - this.BUY_FEE);
    const yourTokens = this.calculateBuyTokens(yourBuy, currentSolInCurve);
    currentSolInCurve += yourSolAfterFee;
    const otherSolAfterFee = otherBuy * (1 - this.BUY_FEE);
    const otherTokens = this.calculateBuyTokens(otherBuy, currentSolInCurve);
    currentSolInCurve += otherSolAfterFee;
    const sellProceeds = this.calculateSellProceeds(yourTokens, currentSolInCurve);
    const profit = sellProceeds - yourBuy;
    const profitPercent = (profit / yourBuy) * 100;

    return {
      yourTokens: Math.round(yourTokens),
      sellProceeds: Number(sellProceeds.toFixed(6)),
      profit: Number(profit.toFixed(6)),
      profitPercent: Number(profitPercent.toFixed(2)),
      finalSolInCurve: currentSolInCurve,
      currentPrice: Number(this.getCurrentPrice(currentSolInCurve).toFixed(9)),
    };
  }

  estimateCreatorSells(scenario: any) {
    const { creatorBuy, yourBuy } = scenario;

    let currentSolInCurve = 0;

    const creatorSolAfterFee = creatorBuy * (1 - this.BUY_FEE);
    const creatorTokens = this.calculateBuyTokens(creatorBuy, currentSolInCurve);
    currentSolInCurve += creatorSolAfterFee;
    const yourSolAfterFee = yourBuy * (1 - this.BUY_FEE);
    const yourTokens = this.calculateBuyTokens(yourBuy, currentSolInCurve);
    currentSolInCurve += yourSolAfterFee;
    const creatorSellProceeds = this.calculateSellProceeds(creatorTokens, currentSolInCurve);

    // Remove SOL from curve (creator's sell removes liquidity)
    const solRemovedFromCurve = creatorTokens * this.getCurrentPrice(currentSolInCurve);
    currentSolInCurve -= solRemovedFromCurve;

    const yourSellProceeds = this.calculateSellProceeds(yourTokens, currentSolInCurve);
    const yourProfit = yourSellProceeds - yourBuy;
    const yourProfitPercent = (yourProfit / yourBuy) * 100;

    return {
      yourTokens: Math.round(yourTokens),
      yourSellProceeds: Number(yourSellProceeds.toFixed(6)),
      yourProfit: Number(yourProfit.toFixed(6)),
      yourProfitPercent: Number(yourProfitPercent.toFixed(2)),
      finalSolInCurve: Number(currentSolInCurve.toFixed(6)),
      creatorSellProceeds: Number(creatorSellProceeds.toFixed(6)),
    };
  }

  // Quick estimation function
  quickEstimate(creatorBuy: number, yourBuy: number, otherBuy: number) {
    const result = this.estimateSellProceeds({
      creatorBuy,
      yourBuy,
      otherBuy,
    });

    return `You'd get ${result.sellProceeds} SOL (${result.profit >= 0 ? '+' : ''}${result.profit} SOL profit, ${result.profitPercent}% return)`;
  }
}

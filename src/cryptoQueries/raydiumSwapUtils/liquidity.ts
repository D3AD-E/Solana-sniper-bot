import {
  GetStructureSchema,
  Liquidity,
  LiquidityPoolKeys,
  LiquidityStateV4,
  MAINNET_PROGRAM_ID,
  MARKET_STATE_LAYOUT_V3,
  Market,
  Percent,
  TOKEN_PROGRAM_ID,
  Token,
  TokenAmount,
  jsonInfo2PoolKeys,
  publicKey,
  struct,
} from '@raydium-io/raydium-sdk';
import { Commitment, Connection, PublicKey } from '@solana/web3.js';
import base58 from 'bs58';
import { existsSync } from 'fs';
import { readFile, writeFile } from 'fs/promises';
import { POOL_FILE_NAME } from '../../constants';
import logger from '../../utils/logger';
export const MINIMAL_MARKET_STATE_LAYOUT_V3 = struct([publicKey('eventQueue'), publicKey('bids'), publicKey('asks')]);
export type MinimalMarketStateLayoutV3 = typeof MINIMAL_MARKET_STATE_LAYOUT_V3;
export type MinimalMarketLayoutV3 = GetStructureSchema<MinimalMarketStateLayoutV3>;
import { Market as SeMarket, OpenOrders } from '@project-serum/serum';

export async function loadPoolKeys() {
  try {
    if (existsSync(POOL_FILE_NAME)) {
      return JSON.parse((await readFile(POOL_FILE_NAME)).toString()) as LiquidityPoolKeys[];
    }

    throw new Error('no file found');
  } catch (error) {
    return await regeneratePoolKeys();
  }
}

export async function regeneratePoolKeys() {
  const liquidityJsonResp = await fetch('https://api.raydium.io/v2/sdk/liquidity/mainnet.json');
  if (!liquidityJsonResp.ok) return [];
  const liquidityJson = (await liquidityJsonResp.json()) as { official: any; unOfficial: any };
  const allPoolKeysJson = [...(liquidityJson?.official ?? []), ...(liquidityJson?.unOfficial ?? [])];

  await writeFile(POOL_FILE_NAME, JSON.stringify(allPoolKeysJson));
  return allPoolKeysJson as LiquidityPoolKeys[];
}

export function findPoolInfoForTokens(allPoolKeysJson: any, mintA: string, mintB: string) {
  const poolData = allPoolKeysJson.find(
    (i: any) => (i.baseMint === mintA && i.quoteMint === mintB) || (i.baseMint === mintB && i.quoteMint === mintA),
  );

  if (!poolData) return null;

  return jsonInfo2PoolKeys(poolData) as LiquidityPoolKeys;
}

export function findPoolInfoForTokensById(allPoolKeysJson: any, id: string) {
  const poolData = allPoolKeysJson.find((i: any) => i.id === id);

  if (!poolData) return null;

  return jsonInfo2PoolKeys(poolData) as LiquidityPoolKeys;
}

export async function calcAmountOut(
  connection: Connection,
  poolKeys: LiquidityPoolKeys,
  rawAmountIn: number,
  slippage: number = 5,
  swapInDirection: boolean,
) {
  const poolInfo = await Liquidity.fetchInfo({ connection, poolKeys });
  let currencyInMint = poolKeys.baseMint;
  let currencyInDecimals = poolInfo.baseDecimals;
  let currencyOutMint = poolKeys.quoteMint;
  let currencyOutDecimals = poolInfo.quoteDecimals;

  if (!swapInDirection) {
    currencyInMint = poolKeys.quoteMint;
    currencyInDecimals = poolInfo.quoteDecimals;
    currencyOutMint = poolKeys.baseMint;
    currencyOutDecimals = poolInfo.baseDecimals;
  }

  const currencyIn = new Token(TOKEN_PROGRAM_ID, currencyInMint, currencyInDecimals);
  const amountIn = new TokenAmount(currencyIn, rawAmountIn.toFixed(currencyInDecimals), false);
  const currencyOut = new Token(TOKEN_PROGRAM_ID, currencyOutMint, currencyOutDecimals);
  const slippageX = new Percent(slippage, 100); // 5% slippage
  const { amountOut, minAmountOut, currentPrice, executionPrice, priceImpact, fee } = Liquidity.computeAmountOut({
    poolKeys,
    poolInfo,
    amountIn,
    currencyOut,
    slippage: slippageX,
  });

  return {
    amountIn,
    amountOut,
    minAmountOut,
    currentPrice,
    executionPrice,
    priceImpact,
    fee,
  };
}

export async function getMinimalMarketV3(
  connection: Connection,
  marketId: PublicKey,
  commitment?: Commitment,
): Promise<MinimalMarketLayoutV3> {
  const marketInfo = await connection.getAccountInfo(marketId, {
    commitment,
    dataSlice: {
      offset: MARKET_STATE_LAYOUT_V3.offsetOf('eventQueue'),
      length: 32 * 3,
    },
  });

  return MINIMAL_MARKET_STATE_LAYOUT_V3.decode(marketInfo!.data);
}

export const RAYDIUM_LIQUIDITY_PROGRAM_ID_V4 = MAINNET_PROGRAM_ID.AmmV4;
export const OPENBOOK_PROGRAM_ID = MAINNET_PROGRAM_ID.OPENBOOK_MARKET;

export function createPoolKeys(
  id: PublicKey,
  accountData: LiquidityStateV4,
  minimalMarketLayoutV3: MinimalMarketLayoutV3,
  market: SeMarket,
): LiquidityPoolKeys {
  return {
    id,
    baseMint: accountData.baseMint,
    quoteMint: accountData.quoteMint,
    lpMint: accountData.lpMint,
    baseDecimals: accountData.baseDecimal, //tonumber
    quoteDecimals: accountData.quoteDecimal,
    lpDecimals: 5,
    version: 4,
    programId: RAYDIUM_LIQUIDITY_PROGRAM_ID_V4,
    authority: Liquidity.getAssociatedAuthority({
      programId: RAYDIUM_LIQUIDITY_PROGRAM_ID_V4,
    }).publicKey,
    openOrders: accountData.openOrders,
    targetOrders: accountData.targetOrders,
    baseVault: accountData.baseVault,
    quoteVault: accountData.quoteVault,
    marketVersion: 3,
    marketProgramId: accountData.marketProgramId,
    marketId: accountData.marketId,
    marketAuthority: Market.getAssociatedAuthority({
      programId: accountData.marketProgramId,
      marketId: accountData.marketId,
    }).publicKey,
    marketBaseVault: market.decoded.baseVault,
    marketQuoteVault: market.decoded.quoteVault,
    marketBids: minimalMarketLayoutV3.bids,
    marketAsks: minimalMarketLayoutV3.asks,
    marketEventQueue: minimalMarketLayoutV3.eventQueue,
    withdrawQueue: accountData.withdrawQueue,
    lpVault: accountData.lpVault,
    lookupTableAccount: PublicKey.default,
  };
}

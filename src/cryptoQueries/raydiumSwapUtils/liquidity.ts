import {
  Liquidity,
  LiquidityPoolKeys,
  Percent,
  TOKEN_PROGRAM_ID,
  Token,
  TokenAmount,
  jsonInfo2PoolKeys,
} from '@raydium-io/raydium-sdk';
import { Connection } from '@solana/web3.js';
import base58 from 'bs58';
import { existsSync } from 'fs';
import { readFile, writeFile } from 'fs/promises';
import { POOL_FILE_NAME } from '../../constants';
import logger from '../../utils/logger';

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

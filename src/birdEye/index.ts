import dotenv from 'dotenv';
import logger from '../utils/logger';

dotenv.config();
export type BirdTokenResponse = {
  success: boolean;
  data?: BirdData;
};

type BirdData = {
  value: number;
};

export const getTokenPrice = async (tokenAddress: string) => {
  const options = { method: 'GET', headers: { 'X-API-KEY': process.env.BIRDEYE_API_KEY! } };
  try {
    const response = await fetch(`https://public-api.birdeye.so/defi/price?address=${tokenAddress}`, options);
    const result = (await response.json()) as BirdTokenResponse;
    if (!result.success || result.data === undefined) {
      logger.error('Fetching token price is broken');
      return undefined;
    }
    return result.data!.value;
  } catch (e) {
    console.error(e);
    return undefined;
  }
};

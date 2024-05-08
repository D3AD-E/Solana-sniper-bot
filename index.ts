import dotenv from 'dotenv';
import logger from './src/utils/logger';
import { getLastUpdatedTokens, getSwapInfo } from './src/browser/scrape';
import listen from './src/listener';
import snipe from './src/sniper';

dotenv.config();
logger.info('Started');
snipe();

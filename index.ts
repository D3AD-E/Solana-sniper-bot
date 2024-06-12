import dotenv from 'dotenv';
import logger from './src/utils/logger';
import snipe from './src/sniper';

dotenv.config();
logger.info('Started');
snipe();

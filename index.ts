import dotenv from 'dotenv';
import logger from './src/utils/logger';
import snipe from './src/pump';

dotenv.config();
logger.info('Started');
snipe();

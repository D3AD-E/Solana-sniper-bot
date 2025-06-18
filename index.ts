import dotenv from 'dotenv';
import logger from './src/utils/logger';
import snipe from './src/pump';
import analize from './src/analize';

dotenv.config();
const args = process.argv.slice(2);
const command = args[0];

async function main() {
  logger.info('Started');

  switch (command) {
    case 'snipe':
      logger.info('Starting snipe mode');
      await snipe();
      break;

    case 'snipe-minimal':
      logger.info('Starting snipe mode minimal func');
      await snipe(true);
      break;

    case 'analyze':
      logger.info('Starting analyze mode');
      await analize();
      break;

    default:
      logger.error('Invalid command. Use: snipe, analyze');
      logger.info('Usage: npm run start <command>');
      logger.info('Commands:');
      logger.info('  snipe   - Run sniper only');
      logger.info('  analyze - Run analyzer only');
      process.exit(1);
  }
}

main();

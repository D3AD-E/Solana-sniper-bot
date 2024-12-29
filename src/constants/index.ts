import { Commitment } from '@solana/web3.js';

export const MAX_SELL_RETRIES = 5;
export const POOL_FILE_NAME = 'pools.json';
export const TOKENS_FILE_NAME = 'tokens.json';
export const LEADERS_FILE_NAME = 'leaders.json';
export const BLACKLIST_FILE_NAME = 'list.json';
export const MAX_REFRESH_DELAY = 3000;
export const MIN_REFRESH_DELAY = 1000;
export const DEFAULT_TRANSACTION_COMMITMENT = 'processed' as Commitment;

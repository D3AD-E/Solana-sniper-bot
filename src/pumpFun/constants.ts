import { PublicKey } from '@solana/web3.js';

/** pump.fun bonding curve program */
export const PUMP_PROGRAM = new PublicKey('6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P');
export const TOKEN_PROGRAM = new PublicKey('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA');
/** every current launch (`create_v2`) mints a token-2022 mint */
export const TOKEN_2022_PROGRAM = new PublicKey('TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb');
export const ASSOCIATED_TOKEN_PROGRAM = new PublicKey('ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL');
export const SYSTEM_PROGRAM = new PublicKey('11111111111111111111111111111111');
export const COMPUTE_BUDGET_PROGRAM = new PublicKey('ComputeBudget111111111111111111111111111111');
export const FEE_PROGRAM = new PublicKey('pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ');

// PDAs with constant seeds, so they never have to be derived at runtime
export const GLOBAL = new PublicKey('4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf');
export const EVENT_AUTHORITY = new PublicKey('Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1');
export const GLOBAL_VOLUME_ACCUMULATOR = new PublicKey('Hq2wp8uJ9jCPsYgNHex8RtqdvMPfVGoYwjvF1ATiwn2Y');
export const FEE_CONFIG = new PublicKey('8Wf5TiAheLUqBrKXeYg2JtAFFMWtKdG2BSFgqUcPVwTt');

// anchor discriminators
export const DISC_BUY = Buffer.from([102, 6, 61, 18, 1, 218, 235, 234]);
export const DISC_CREATE = Buffer.from([24, 30, 200, 40, 5, 28, 7, 119]);
export const DISC_CREATE_V2 = Buffer.from([214, 144, 76, 236, 95, 139, 49, 180]);

export const SEED_BONDING_CURVE = Buffer.from('bonding-curve');
export const SEED_BONDING_CURVE_V2 = Buffer.from('bonding-curve-v2');
export const SEED_CREATOR_VAULT = Buffer.from('creator-vault');
export const SEED_USER_VOLUME_ACCUMULATOR = Buffer.from('user_volume_accumulator');

/**
 * The buy instruction costs ~89k compute units against a token-2022 mint with the current
 * account set, so the old 71,990 unit limit is no longer enough.
 */
export const BUY_COMPUTE_UNIT_LIMIT = 120_000;

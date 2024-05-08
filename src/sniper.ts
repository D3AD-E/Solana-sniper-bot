import {
  TokenAmount,
  Token,
  Liquidity,
  LiquidityPoolKeysV4,
  LiquidityPoolKeys,
  TokenAccount,
  LIQUIDITY_STATE_LAYOUT_V4,
  MARKET_STATE_LAYOUT_V3,
  SPL_MINT_LAYOUT,
  Market,
  TOKEN_PROGRAM_ID,
  LiquidityStateV4,
  MarketStateV3,
} from '@raydium-io/raydium-sdk';
import { AccountLayout, getMint } from '@solana/spl-token';
import { Commitment, Connection, KeyedAccountInfo, PublicKey } from '@solana/web3.js';
import {
  buyJito,
  checkMintable,
  closeAccount,
  createAccount,
  getPoolKeys,
  getTokenAccounts,
  getTokenBalanceSpl,
  preformSwap,
  preformSwapJito,
  saveTokenAccount,
  sellJito,
} from './cryptoQueries';
import logger from './utils/logger';
import { solanaConnection, wallet } from './solana';
import { MAX_REFRESH_DELAY, MIN_REFRESH_DELAY, TOKENS_FILE_NAME } from './constants';
import {
  OPENBOOK_PROGRAM_ID,
  RAYDIUM_LIQUIDITY_PROGRAM_ID_V4,
  findPoolInfoForTokensById,
  loadPoolKeys,
  regeneratePoolKeys,
} from './cryptoQueries/raydiumSwapUtils/liquidity';
import { existsSync } from 'fs';
import { readFile, writeFile } from 'fs/promises';
import { TokenInfo, getLastUpdatedTokens, getSwapInfo } from './browser/scrape';
import { sendMessage } from './telegramBot';
import { getTokenPrice } from './birdEye';
import { ProviderType, getProviderType } from './enums/LiqudityProviderType';
import { BoughtTokenData } from './listener.types';
import bs58 from 'bs58';
import { MinimalTokenAccountData } from './cryptoQueries/cryptoQueries.types';
import { JitoClient } from './jito/searcher';

let existingTokenAccounts: TokenAccount[] = [];

const quoteToken = Token.WSOL;
let swapAmount: TokenAmount;
let quoteTokenAssociatedAddress: PublicKey;
const existingLiquidityPools: Set<string> = new Set<string>();
const existingTokenAccountsExtended: Map<string, MinimalTokenAccountData> = new Map<string, MinimalTokenAccountData>();
const existingOpenBookMarkets: Set<string> = new Set<string>();

export default async function snipe(): Promise<void> {
  logger.info(`Wallet Address: ${wallet.publicKey}`);
  swapAmount = new TokenAmount(Token.WSOL, process.env.SWAP_SOL_AMOUNT, false);

  logger.info(`Swap sol amount: ${swapAmount.toFixed()} ${quoteToken.symbol}`);

  existingTokenAccounts = await getTokenAccounts(
    solanaConnection,
    wallet.publicKey,
    process.env.COMMITMENT as Commitment,
  );

  const tokenAccount = existingTokenAccounts.find(
    (acc) => acc.accountInfo.mint.toString() === quoteToken.mint.toString(),
  )!;

  if (!tokenAccount) {
    throw new Error(`No ${quoteToken.symbol} token account found in wallet: ${wallet.publicKey}`);
  }

  quoteTokenAssociatedAddress = tokenAccount.pubkey;
  await listenToChanges();
}

export async function processRaydiumPool(id: PublicKey, poolState: LiquidityStateV4) {
  const poolSize = new TokenAmount(quoteToken, poolState.swapQuoteInAmount, true);
  logger.info(`Processing pool: ${id.toString()} with ${poolSize.toFixed()} ${quoteToken.symbol} in liquidity`);

  await buyJito(id, poolState, existingTokenAccountsExtended, quoteTokenAssociatedAddress);
  let currentPrice = (await getTokenPrice(poolState.baseMint.toString())) ?? 0;
  logger.info(`Price, ${currentPrice}`);
}

export async function processOpenBookMarket(updatedAccountInfo: KeyedAccountInfo) {
  let accountData: MarketStateV3 | undefined;
  try {
    accountData = MARKET_STATE_LAYOUT_V3.decode(updatedAccountInfo.accountInfo.data);

    if (existingTokenAccountsExtended.has(accountData.baseMint.toString())) {
      return;
    }
    const token = saveTokenAccount(accountData.baseMint, accountData);
    existingTokenAccountsExtended.set(accountData.baseMint.toString(), token);
  } catch (e) {
    logger.debug(e);
  }
}

async function listenToChanges() {
  let processing = false;
  const runTimestamp = Math.floor(new Date().getTime() / 1000);

  const raydiumSubscriptionId = solanaConnection.onProgramAccountChange(
    RAYDIUM_LIQUIDITY_PROGRAM_ID_V4,
    async (updatedAccountInfo) => {
      if (processing) return;
      const key = updatedAccountInfo.accountId.toString();
      const poolState = LIQUIDITY_STATE_LAYOUT_V4.decode(updatedAccountInfo.accountInfo.data);
      const poolOpenTime = parseInt(poolState.poolOpenTime.toString());
      const existing = existingLiquidityPools.has(key);

      if (poolOpenTime > runTimestamp && !existing) {
        processing = true;
        existingLiquidityPools.add(key);
        const _ = processRaydiumPool(updatedAccountInfo.accountId, poolState);
      }
    },
    process.env.COMMITMENT as Commitment,
    [
      { dataSize: LIQUIDITY_STATE_LAYOUT_V4.span },
      {
        memcmp: {
          offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('quoteMint'),
          bytes: quoteToken.mint.toBase58(),
        },
      },
      {
        memcmp: {
          offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('marketProgramId'),
          bytes: OPENBOOK_PROGRAM_ID.toBase58(),
        },
      },
      {
        memcmp: {
          offset: LIQUIDITY_STATE_LAYOUT_V4.offsetOf('status'),
          bytes: bs58.encode([6, 0, 0, 0, 0, 0, 0, 0]),
        },
      },
    ],
  );

  const openBookSubscriptionId = solanaConnection.onProgramAccountChange(
    OPENBOOK_PROGRAM_ID,
    async (updatedAccountInfo) => {
      const key = updatedAccountInfo.accountId.toString();
      const existing = existingOpenBookMarkets.has(key);
      if (!existing) {
        existingOpenBookMarkets.add(key);
        const _ = processOpenBookMarket(updatedAccountInfo);
      }
    },
    process.env.COMMITMENT as Commitment,
    [
      { dataSize: MARKET_STATE_LAYOUT_V3.span },
      {
        memcmp: {
          offset: MARKET_STATE_LAYOUT_V3.offsetOf('quoteMint'),
          bytes: quoteToken.mint.toBase58(),
        },
      },
    ],
  );
  const walletSubscriptionId = solanaConnection.onProgramAccountChange(
    TOKEN_PROGRAM_ID,
    async (updatedAccountInfo) => {
      const accountData = AccountLayout.decode(updatedAccountInfo.accountInfo!.data);

      if (updatedAccountInfo.accountId.equals(quoteTokenAssociatedAddress)) {
        return;
      }
      logger.info(`Selling?`);
      await new Promise((resolve) => setTimeout(resolve, 60000));
      const _ = sellJito(
        accountData.mint,
        accountData.amount,
        existingTokenAccountsExtended,
        quoteTokenAssociatedAddress,
      );
    },
    process.env.COMMITMENT as Commitment,
    [
      {
        dataSize: 165,
      },
      {
        memcmp: {
          offset: 32,
          bytes: wallet.publicKey.toBase58(),
        },
      },
    ],
  );
}

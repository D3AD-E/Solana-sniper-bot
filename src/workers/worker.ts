import { parentPort, workerData } from 'worker_threads';
import WebSocket from 'ws';
import logger from '../utils/logger';
import { RawAccount } from '@solana/spl-token';
import { TransactionConfirmationStrategy, Commitment, PublicKey } from '@solana/web3.js';
import BigNumber from 'bignumber.js';
import { sell, getTokenAccounts, confirmTransactionInTimeframe } from '../cryptoQueries';
import { solanaConnection, wallet } from '../solana';
import { MinimalTokenAccountData } from '../cryptoQueries/cryptoQueries.types';
import { ParentMessage, WorkerAction, WorkerMessage, WorkerResult } from './worker.types';
import { fromSerializable } from './converter';
process.env = { ...process.env, ...workerData };
let wsPairs: WebSocket | undefined = undefined;

let lastRequest: any | undefined = undefined;
let currentTokenKey = '';
let foundTokenData: RawAccount | undefined = undefined;
let bignumberInitialPrice: BigNumber | undefined = undefined;
let bignumberInitialPriceStart: BigNumber | undefined = undefined;
let minimalAccount: MinimalTokenAccountData | undefined = undefined;
const timeoutSec = Number(workerData.SELL_TIMEOUT_SEC!);

const stopLossPrecents = Number(workerData.STOP_LOSS_PERCENTS!) * -1;
const takeProfitPercents = Number(workerData.TAKE_PROFIT_PERCENTS!);
let quoteTokenAssociatedAddress: PublicKey;
let sentBuyTime: Date | undefined = undefined;
let timeoutId: NodeJS.Timeout | undefined = undefined;
let incactivityTimeoutId: NodeJS.Timeout | undefined = undefined;
let priceTimeoutId: NodeJS.Timeout | undefined = undefined;
let percentageGainNumber: number | undefined = undefined;

function setupPairSocket() {
  wsPairs = new WebSocket(workerData.GEYSER_ENDPOINT!);
  wsPairs.on('open', function open() {
    if (lastRequest) {
      lastRequest = {
        jsonrpc: '2.0',
        id: 420,
        method: 'transactionSubscribe',
        params: [
          {
            vote: false,
            failed: false,
            accountInclude: [currentTokenKey],
          },
          {
            commitment: 'processed',
            encoding: 'jsonParsed',
            transactionDetails: 'full',
            showRewards: false,
            maxSupportedTransactionVersion: 1,
          },
        ],
      };
      wsPairs!.send(JSON.stringify(lastRequest));
    }
  });
  wsPairs.on('message', async function incoming(data) {
    const messageStr = data.toString();
    try {
      var jData = JSON.parse(messageStr);
      const instructionWithSwapSell = jData?.params?.result?.transaction?.meta?.innerInstructions[0];
      if (instructionWithSwapSell !== undefined) {
        getSwappedAmounts(instructionWithSwapSell, jData?.params?.result?.transaction?.transaction?.signatures[0]);
      }
      const instructionWithSwapBuy =
        jData?.params?.result?.transaction?.meta?.innerInstructions[
          jData?.params?.result?.transaction?.meta?.innerInstructions.length - 1
        ];
      if (instructionWithSwapBuy !== undefined) {
        getSwappedAmounts(instructionWithSwapBuy, jData?.params?.result?.transaction?.transaction?.signatures[0]);
      }
      const instructionWithSwapBuy2 =
        jData?.params?.result?.transaction?.meta?.innerInstructions[
          jData?.params?.result?.transaction?.meta?.innerInstructions.length - 2
        ];
      if (instructionWithSwapBuy2 !== undefined) {
        getSwappedAmounts(instructionWithSwapBuy2, jData?.params?.result?.transaction?.transaction?.signatures[0]);
      }
    } catch (e) {
      console.log(messageStr);
      console.error('Failed to parse JSON:', e);
      if (foundTokenData) sellOnActionGeyser(foundTokenData);
      setupPairSocket();
    }
  });
  wsPairs.on('error', function error(err) {
    console.error('WebSocket error:', err);
  });
  wsPairs.on('close', async function close() {
    logger.warn('WebSocket is closed pair');

    // throw 'Websocket closed';
    await new Promise((resolve) => setTimeout(resolve, 200));
    setupPairSocket();
  });
}

async function getSwappedAmounts(instructionWithSwap: any, signature: any) {
  const swapDataBuy = instructionWithSwap.instructions?.filter((x: any) => x.parsed?.info.amount !== undefined);
  if (swapDataBuy !== undefined) {
    const sol = swapDataBuy.find(
      (x: any) => x.parsed.info.authority !== '5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1',
    );
    if (sol) {
      const other = swapDataBuy.find(
        (x: any) => x.parsed.info.authority === '5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1',
      );
      if (sol === undefined || other === undefined) {
        logger.warn(`Geyser is broken`);
        if (foundTokenData) sellOnActionGeyser(foundTokenData!);
        return;
      }
      let price = BigNumber(sol.parsed.info.amount as string).div(other.parsed.info.amount as string);
      const isSolSwapped = price.gt(1);
      if (isSolSwapped) price = BigNumber(other.parsed.info.amount as string).div(sol.parsed.info.amount as string);
      if (!bignumberInitialPriceStart) {
        console.log(signature.toString());
        bignumberInitialPriceStart = price;
      }
      if (foundTokenData) {
        if (sol === undefined || other === undefined) {
          logger.warn(`Geyser is broken, selling`);
          sellOnActionGeyser(foundTokenData!);
          return;
        }
        if (incactivityTimeoutId) clearTimeout(incactivityTimeoutId);
        if (!bignumberInitialPrice) {
          const percentageGain = price
            .minus(bignumberInitialPriceStart)
            .div(bignumberInitialPriceStart)
            .multipliedBy(100);
          const priceFromStart = Number(percentageGain.toFixed(5));
          console.log(priceFromStart, signature.toString(), price.toString(), bignumberInitialPriceStart.toString());
          if (priceFromStart < -5) {
            logger.info(`Selling at PRICEDROP, change addr ${foundTokenData!.mint.toString()}`);
            sellOnActionGeyser(foundTokenData!);
            return;
          }
          bignumberInitialPrice = price;
          return;
        }

        const percentageGain = price.minus(bignumberInitialPrice).div(bignumberInitialPrice).multipliedBy(100);
        percentageGainNumber = Number(percentageGain.toFixed(5));
        const now = new Date();
        if (now.getTime() - sentBuyTime!.getTime() > 30 * 1000) {
          incactivityTimeoutId = setTimeout(() => {
            if (!foundTokenData) return;
            incactivityTimeoutId = undefined;
            logger.info(`Selling at INACTIVITY, change addr ${foundTokenData!.mint.toString()}`);
            sellOnActionGeyser(foundTokenData!);
            return;
          }, 15 * 1000);
        }

        if (Number(percentageGain.toFixed(5)) < 0) logger.warn(percentageGain.toString() + ' ' + signature.toString());
        else logger.info(percentageGain.toString() + ' ' + signature.toString());

        if (percentageGainNumber <= stopLossPrecents) {
          if (!foundTokenData) return;
          logger.warn(`Selling at LOSS, loss ${percentageGainNumber}%, addr ${foundTokenData!.mint.toString()}`);
          await sellOnActionGeyser(foundTokenData!);
          return;
        }
        if (percentageGainNumber >= takeProfitPercents) {
          if (!foundTokenData) return;
          logger.info(
            `Selling at TAKEPROFIT, increase ${percentageGainNumber}%, addr ${foundTokenData!.mint.toString()}`,
          );
          await sellOnActionGeyser(foundTokenData!);

          return;
        }
      }
    }
  }
}

function clearAfterSell() {
  lastRequest = undefined;
  if (wsPairs?.readyState === wsPairs?.OPEN) wsPairs?.close();

  foundTokenData = undefined;
  bignumberInitialPrice = undefined;
  minimalAccount = undefined;
  sentBuyTime = undefined;
  bignumberInitialPriceStart = undefined;
  percentageGainNumber = undefined;
  const message: ParentMessage = {
    result: WorkerResult.SellSuccess,
    data: {
      token: currentTokenKey,
    },
  };
  currentTokenKey = '';
  parentPort!.postMessage(message);
}

async function sellOnActionGeyser(account: RawAccount) {
  if (timeoutId) clearTimeout(timeoutId);
  if (priceTimeoutId) clearTimeout(priceTimeoutId);
  if (incactivityTimeoutId) clearTimeout(incactivityTimeoutId);

  bignumberInitialPrice = undefined;
  foundTokenData = undefined;
  let confirmation = false;
  let tries = 0;
  while (!confirmation && tries < 5) {
    const signature = await sell(account.amount, minimalAccount!, quoteTokenAssociatedAddress, account.mint.toString());
    confirmation = await confirmTransactionInTimeframe(signature!);
    if (!confirmation) {
      logger.warn('Sent sell but it failed');
      const existingTokenAccounts = await getTokenAccounts(
        solanaConnection,
        wallet.publicKey,
        workerData.COMMITMENT as Commitment,
      );
      let tokenAccount = existingTokenAccounts.find(
        (acc) => acc.accountInfo.mint.toString() === account.mint.toString(),
      );
      if (!tokenAccount) {
        logger.info('Sell success');
        clearAfterSell();
        return;
      }
      //accountinfo undefined???
      if (tokenAccount.accountInfo === undefined) {
        const existingTokenAccounts = await getTokenAccounts(
          solanaConnection,
          wallet.publicKey,
          workerData.COMMITMENT as Commitment,
        );
        tokenAccount = existingTokenAccounts.find((acc) => acc.accountInfo.mint.toString() === account.mint.toString());
      }
      const signature = await sell(
        tokenAccount!.accountInfo.amount,
        minimalAccount!,
        quoteTokenAssociatedAddress,
        account.mint.toString(),
      );
      confirmation = await confirmTransactionInTimeframe(signature!);
      tries++;
    } else {
      logger.info('Sell success');
      clearAfterSell();
      return;
    }
  }
  clearAfterSell();
}

// to worker
parentPort?.on('message', (data: string) => {
  const message = fromSerializable(JSON.parse(data)) as WorkerMessage;
  if (message.action === WorkerAction.Setup) {
    quoteTokenAssociatedAddress = message.data!.quoteTokenAssociatedAddress!;
    setupPairSocket();
  } else if (message.action === WorkerAction.GetToken) {
    (currentTokenKey = message.data!.token!), (lastRequest = message.data!.lastRequest!);
    sentBuyTime = new Date();
    if (wsPairs?.readyState === wsPairs?.OPEN) wsPairs!.send(JSON.stringify(lastRequest));
  } else if (message.action === WorkerAction.ForceSell) {
    sellOnActionGeyser(message.data!.accountData!);
  } else if (message.action === WorkerAction.GotWalletToken) {
    const now = new Date();
    if (now.getTime() - sentBuyTime!.getTime() > 50 * 1000) {
      logger.warn('Buy took too long, selling');
      sellOnActionGeyser(message.data!.foundTokenData!);
      return;
    }
    foundTokenData = message.data!.foundTokenData!;
    timeoutId = setTimeout(() => {
      if (!foundTokenData) return;
      timeoutId = undefined;
      logger.info(`Selling at TIMEOUT, change addr ${foundTokenData!.mint.toString()}`);
      sellOnActionGeyser(foundTokenData!);
      return;
    }, timeoutSec * 1000);

    priceTimeoutId = setTimeout(() => {
      if (!foundTokenData) return;
      if (percentageGainNumber && percentageGainNumber < 1) {
        priceTimeoutId = undefined;
        logger.info(`Selling at PRICETIMEOUT, change addr ${foundTokenData!.mint.toString()}`);
        sellOnActionGeyser(foundTokenData!);
      }
      return;
    }, 50 * 1000);
  } else if (message.action === WorkerAction.AddTokenAccount) {
    minimalAccount = message.data!.tokenAccount!;
  } else if (message.action === WorkerAction.Clear) {
    clearAfterSell();
  }
});

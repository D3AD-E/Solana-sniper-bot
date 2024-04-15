import { Page } from 'puppeteer';
import logger from '../utils/logger';
import { DEX_TOOLS_BASE_URL, DEX_TOOLS_SOLANA_URL } from './constants';
import { HeadlessBrowser } from './headlessBrowser';

export type TokenInfo = {
  symbol: string;
  url: string;
};
//top-list
const browser = new HeadlessBrowser();
let mainRefereshPage: Page | undefined;

export const getLastUpdatedTokens = async (): Promise<TokenInfo[] | undefined> => {
  if (mainRefereshPage === undefined) {
    mainRefereshPage = await browser.createPage();
    await browser.gotoUrl(mainRefereshPage, DEX_TOOLS_SOLANA_URL);
  } else {
    await mainRefereshPage.reload({ waitUntil: 'networkidle0' });
  }

  const data = await mainRefereshPage.evaluate(() => {
    // Your scraping code here
    const topList = document.getElementsByClassName('top-list');
    const topListArray = Array.from(topList);
    if (topListArray.length === 0) return undefined;
    const topListData = topListArray[0];
    const links = topListData.querySelectorAll('a');
    const linksArray = Array.from(links);
    const symbols = topListData.getElementsByClassName('symbol');
    const symbolsArray = Array.from(symbols);
    const linksToDex = linksArray.filter((link) => link.href.startsWith('https://www.dextools.io'));

    return symbolsArray.map((symbol, index) => {
      return {
        symbol: symbol.innerHTML,
        url: linksToDex[index].href,
      };
    });
  });
  return data;
};

export const getSwapInfo = async (url: string) => {
  const page = await browser.createPage();
  await browser.gotoUrl(page, url);

  const data = await page.evaluate(() => {
    // Your scraping code here
    const tokenInfo = document.getElementsByClassName('token-pair-info');
    const tokenInfoArray = Array.from(tokenInfo);
    if (tokenInfoArray.length === 0) return undefined;
    const tokenInfoData = tokenInfoArray[0];
    const links = tokenInfoData.querySelectorAll('a');
    const linksArray = Array.from(links);
    const price = document.getElementsByClassName('big-price');
    const prices = Array.from(price);
    const actualPrice = prices[0].textContent!.split(' ')[1];
    console.log(actualPrice);
    if (Number.isNaN(Number(actualPrice))) return undefined;
    const linksToRet = linksArray
      .filter((link) => link.href.startsWith('https:'))
      .map((link) => {
        const parts = link.href.split('/');
        const lastPart = parts[parts.length - 1];
        return {
          id: lastPart,
        };
      });
    return {
      links: linksToRet,
      initialPrice: Number(actualPrice),
    };
  });
  page.close();
  return data;
};

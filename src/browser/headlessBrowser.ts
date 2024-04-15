import { EventEmitter } from 'events';
import * as Puppeteer from 'puppeteer';
import { PuppeteerLaunchOptions } from 'puppeteer';
import logger from '../utils/logger';
import UserAgent from 'user-agents';
import { MAX_DELAY, MIN_DELAY } from './constants';

export class HeadlessBrowser extends EventEmitter {
  private browser?: Puppeteer.Browser;
  private lastRequestDate?: Date;
  private shouldRequestFrom?: Date;

  public async createPage() {
    if (!this.browser) {
      await this.initBrowser();
    }
    const page = await this.browser!.newPage();

    // Avoiding Bot detection
    await page.setUserAgent(new UserAgent().toString());

    // page.on('console', (msg) => console.log(msg.text()));
    return page;
  }

  public async gotoUrl(page: Puppeteer.Page, url: string) {
    const now = new Date();
    if (this.shouldRequestFrom !== undefined) {
      if (this.shouldRequestFrom! > now) {
        await new Promise((resolve) => setTimeout(resolve, this.shouldRequestFrom!.getTime() - now.getTime()));
      }
    }
    this.lastRequestDate = now;
    const randomInterval = Math.random() * (MAX_DELAY - MIN_DELAY) + MIN_DELAY;
    this.shouldRequestFrom = new Date(this.lastRequestDate.getTime() + randomInterval);
    await page.goto(url, {
      timeout: 300000,
      waitUntil: 'networkidle0',
    });
    return page;
  }

  public closeBrowser() {
    if (this.browser) {
      this.browser.close();
    }
  }

  private async initBrowser() {
    // const browserArgs: PuppeteerLaunchOptions = {
    //   args: [
    //     '--no-sandbox',
    //     '--disable-setuid-sandbox',
    //     '--headless',
    //     '--disable-gpu',
    //     '--disable-dev-shm-usage',
    //     '--disable-web-security',
    //     '--disable-infobars',
    //     '--window-position=0,0',
    //     '--ignore-certifcate-errors',
    //     '--ignore-certifcate-errors-spki-list',
    //   ],
    // };
    this.browser = await Puppeteer.launch();
  }
}

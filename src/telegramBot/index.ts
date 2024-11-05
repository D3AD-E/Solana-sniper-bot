import TelegramBot from 'node-telegram-bot-api';
import dotenv from 'dotenv';
import logger from '../utils/logger';
import { exit } from 'process';
import eventEmitter from '../eventEmitter';
import { USER_STOP_EVENT } from '../eventEmitter/eventEmitter.consts';

dotenv.config();

const bot = new TelegramBot(process.env.BOT_TOKEN!, { polling: true });

bot.on('message', (msg) => {
  console.log(msg?.text);
  if (msg?.text === 'exit') {
    eventEmitter.emit(USER_STOP_EVENT, {});
  }
});

export const sendMessage = (text: string) => {
  try {
    bot.sendMessage(Number(process.env.CHAT_ID!), text);
  } catch (e) {
    console.error(e);
  }
};

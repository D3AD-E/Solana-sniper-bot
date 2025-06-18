import { SlotList } from 'jito-ts/dist/gen/block-engine/searcher';
import { LEADERS_FILE_NAME } from '../constants';
import { JitoClient } from './searcher';
import { readFile, writeFile } from 'fs/promises';

export async function refreshLeaders() {
  const j = await JitoClient.getInstance();
  const l = await j.getConnectedLeaders();
  await writeFile(LEADERS_FILE_NAME, JSON.stringify(l));
}

export async function readLeaders() {
  const data = JSON.parse((await readFile(LEADERS_FILE_NAME)).toString()) as {
    [key: string]: SlotList;
  };
  return data;
}

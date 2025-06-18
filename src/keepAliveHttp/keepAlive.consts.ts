import { Region } from './keepAliveHttp.types';

export const SLOT_ENDPOINT_BY_REGION: Record<Region, string> = {
  [Region.Frankfurt]: 'de1.0slot.trade',
  [Region.NY]: 'ny1.0slot.trade',
  [Region.Tokyo]: 'jp.0slot.trade',
  [Region.Amsterdam]: 'ams1.0slot.trade',
  [Region.LosAngeles]: 'la1.0slot.trade',
};

export const ASTRA_ENDPOINT_BY_REGION: Partial<Record<Region, string>> = {
  [Region.Frankfurt]: 'fr.gateway.astralane.io',
  [Region.NY]: 'ny.gateway.astralane.io',
  [Region.Tokyo]: 'jp.gateway.astralane.io',
  [Region.Amsterdam]: 'ams.gateway.astralane.io',
};

export const NODE1_ENDPOINT_BY_REGION: Partial<Record<Region, string>> = {
  [Region.NY]: 'ny.node1.me',
  [Region.Tokyo]: 'ny.node1.me',
  [Region.Amsterdam]: 'ams.node1.me',
  [Region.Frankfurt]: 'fra.node1.me',
};

export const NEXTBLOCK_ENDPOINT_BY_REGION: Partial<Record<Region, string>> = {
  [Region.Tokyo]: 'tokyo.nextblock.io',
  [Region.Frankfurt]: 'fra.nextblock.io',
  [Region.NY]: 'ny.nextblock.io',
};

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
  [Region.LosAngeles]: 'la.gateway.astralane.io',
};

/**
 * node1 publishes ny, ams and fra only: tyo/tokyo/jp/la do not resolve. Tokyo used to be
 * mapped to 'ny.node1.me', which silently sent every Tokyo transaction across the Pacific.
 * The entry is dropped instead, so `endpointForRegion` falls back explicitly and the gap in
 * coverage is visible rather than disguised as a Tokyo endpoint.
 */
export const NODE1_ENDPOINT_BY_REGION: Partial<Record<Region, string>> = {
  [Region.NY]: 'ny.node1.me',
  [Region.Amsterdam]: 'ams.node1.me',
  [Region.Frankfurt]: 'fra.node1.me',
};

export const NEXTBLOCK_ENDPOINT_BY_REGION: Partial<Record<Region, string>> = {
  [Region.Tokyo]: 'tokyo.nextblock.io',
  [Region.Frankfurt]: 'fra.nextblock.io',
  [Region.NY]: 'ny.nextblock.io',
};

export const JITO_ENDPOINT_BY_REGION: Partial<Record<Region, string>> = {
  [Region.Frankfurt]: 'frankfurt.mainnet.block-engine.jito.wtf',
  [Region.NY]: 'ny.mainnet.block-engine.jito.wtf',
  [Region.Tokyo]: 'tokyo.mainnet.block-engine.jito.wtf',
  [Region.Amsterdam]: 'amsterdam.mainnet.block-engine.jito.wtf',
  [Region.LosAngeles]: 'slc.mainnet.block-engine.jito.wtf',
};

/**
 * Region coverage is uneven: 0slot, astra and jito serve all five regions, node1 has no
 * Tokyo or LA endpoint, and nextblock only publishes three. A region without an entry falls
 * back to the nearest one that exists, so a gap degrades latency instead of dropping the
 * provider.
 */
export const REGION_FALLBACK: Record<Region, Region[]> = {
  [Region.Frankfurt]: [Region.Amsterdam, Region.NY],
  [Region.Amsterdam]: [Region.Frankfurt, Region.NY],
  [Region.NY]: [Region.Amsterdam, Region.Frankfurt],
  [Region.Tokyo]: [Region.LosAngeles, Region.NY],
  [Region.LosAngeles]: [Region.NY, Region.Tokyo],
};

export function endpointForRegion(
  table: Partial<Record<Region, string>>,
  region: Region,
): string | undefined {
  const direct = table[region];
  if (direct) return direct;
  for (const fallback of REGION_FALLBACK[region] ?? []) {
    const host = table[fallback];
    if (host) return host;
  }
  return undefined;
}

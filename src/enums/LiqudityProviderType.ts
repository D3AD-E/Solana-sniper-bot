import { enumFromStringValue } from '../utils/enumHelper';

export enum ProviderType {
  Unknown = 'Unk',
  Raydium = 'Raydium',
}

export const getProviderType = (provider: string) => {
  const expectedType = enumFromStringValue(ProviderType, provider);
  if (expectedType === undefined) return ProviderType.Unknown;
  return expectedType;
};

export enum TxType {
  Default,
  Small,
  NodeOne,
}
export type PositionPatch = {
  mintPos: number;
  curvePos: number;
  ownerVaultPos: number;
  associatedUserPos: number;
  bondingPos: number;
  amountPos: number;
  noncePos: number;
  tipAmountPos: number;
  tipAccountPos: number;
  cuPricePos: number;
};
export type TxTemplate = {
  tx: Buffer;
  pos: PositionPatch;
};

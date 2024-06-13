import { PublicKey } from '@solana/web3.js';
import BigNumber from 'bignumber.js';
export function toSerializable(obj: any): any {
  if (obj instanceof PublicKey) {
    return { _type: 'PublicKey', value: obj.toBase58() };
  } else if (obj instanceof BigNumber) {
    return { _type: 'BigNumber', value: obj.toString() };
  } else if (Array.isArray(obj)) {
    return obj.map(toSerializable);
  } else if (typeof obj === 'bigint') {
    return { _type: 'BigInt', value: obj.toString() };
  } else if (typeof obj === 'object' && obj !== null) {
    return Object.fromEntries(Object.entries(obj).map(([k, v]) => [k, toSerializable(v)]));
  } else {
    return obj;
  }
}

// Restore objects from their plain serialized form
export function fromSerializable(obj: any): any {
  if (obj && obj._type === 'PublicKey') {
    return new PublicKey(obj.value);
  } else if (obj && obj._type === 'BigNumber') {
    return new BigNumber(obj.value);
  } else if (obj && obj._type === 'BigInt') {
    return BigInt(obj.value);
  } else if (Array.isArray(obj)) {
    return obj.map(fromSerializable);
  } else if (typeof obj === 'object' && obj !== null) {
    return Object.fromEntries(Object.entries(obj).map(([k, v]) => [k, fromSerializable(v)]));
  } else {
    return obj;
  }
}

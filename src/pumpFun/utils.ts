import { Keypair } from '@solana/web3.js';
import { PublicKey } from '@solana/web3.js';
import * as nacl from 'tweetnacl';
import { sha256 } from 'js-sha256';
import * as ed from '@noble/ed25519';
import { wallet } from '../solana';
import { TxType } from './pumpFun.types';
const tipAmount = Number(process.env.JITO_TIP!);

export function patchBigIntLE(buffer: Buffer, offset: number, value: bigint) {
  const tmp = Buffer.alloc(8); // 64 bits
  tmp.writeBigUInt64LE(BigInt(value));
  tmp.copy(buffer, offset); // Copy 8 bytes into main buffer at offset
}

export function patchPublicKeyAt(buf: Buffer, offset: number, pubkey: PublicKey) {
  pubkey.toBuffer().copy(buf, offset);
}

export function replaceAt(str: string, start: number, replacement: string) {
  const end = start + replacement.length;
  return str.slice(0, start) + replacement + str.slice(end);
}

export function findAllIndexesInString(str: string, substring: string) {
  const indexes = [];
  let i = 0;

  while ((i = str.indexOf(substring, i)) !== -1) {
    indexes.push(i);
    i += 1; // move forward to find overlapping matches too
  }
  if (indexes.length === 0) return [-1];
  return indexes;
}

export function signMessageBytes(message: Buffer, signer: Keypair): Buffer {
  const sig = nacl.sign.detached(message, signer.secretKey); // ← raw bytes
  return Buffer.from(sig); // 64-byte Buffer
}

export function encodeSigCount(count: number): Buffer {
  if (count > 127) throw new Error('More than 127 sigs not supported here');
  return Buffer.from([count]);
}

export function buildRawTx(message: Buffer, sigs: Buffer[]): Buffer {
  return Buffer.concat([encodeSigCount(sigs.length), ...sigs, message]);
}

// Convert Keypair to raw secret/public
const privateKey = wallet.secretKey.slice(0, 32);
const publicKey = wallet.publicKey.toBytes();

// Sign buffer
export async function fastSignMessage(message: Uint8Array) {
  const sig = await ed.sign(message, privateKey);
  return Buffer.from(sig); // 64-byte Buffer
}

export function getLamportsBasedOnTxType(txType: TxType) {
  switch (txType) {
    case TxType.Default:
      return tipAmount * 105;
    case TxType.Small:
      return tipAmount * 20;
    case TxType.NodeOne:
      return tipAmount * 21;
  }
}

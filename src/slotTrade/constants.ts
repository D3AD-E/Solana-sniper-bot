import { LAMPORTS_PER_SOL, PublicKey } from '@solana/web3.js';

export const SLOT_ACCOUNTS = [
  new PublicKey('Eb2KpSC8uMt9GmzyAEm5Eb1AAAgTjRaXWFjKyFXHZxF3'),
  new PublicKey('FCjUJZ1qozm1e8romw216qyfQMaaWKxWsuySnumVCCNe'),
  new PublicKey('ENxTEjSQ1YabmUpXAdCgevnHQ9MHdLv8tzFiuiYJqa13'),
  new PublicKey('6rYLG55Q9RpsPGvqdPNJs4z5WTxJVatMB8zV3WJhs5EK'),
  new PublicKey('Cix2bHfqPcKcM233mzxbLk14kSggUUiz2A87fJtGivXr'),
];

export function getRandomSlotAccount() {
  const randomIndex = Math.floor(Math.random() * SLOT_ACCOUNTS.length);
  return SLOT_ACCOUNTS[randomIndex];
}

export const NODE_ONE_ACCOUNTS = [
  new PublicKey('node1PqAa3BWWzUnTHVbw8NJHC874zn9ngAkXjgWEej'),
  new PublicKey('node1UzzTxAAeBTpfZkQPJXBAqixsbdth11ba1NXLBG'),
  new PublicKey('node1Qm1bV4fwYnCurP8otJ9s5yrkPq7SPZ5uhj3Tsv'),
  new PublicKey('node1PUber6SFmSQgvf2ECmXsHP5o3boRSGhvJyPMX1'),
  new PublicKey('node1AyMbeqiVN6eoQzEAwCA6Pk826hrdqdAHR7cdJ3'),
  new PublicKey('node1YtWCoTwwVYTFLfS19zquRQzYX332hs1HEuRBjC'),
  new PublicKey('node1FdMPnJBN7QTuhzNw3VS823nxFuDTizrrbcEqzp'),
];

export function getRandomNodeAccount() {
  const randomIndex = Math.floor(Math.random() * NODE_ONE_ACCOUNTS.length);
  return NODE_ONE_ACCOUNTS[randomIndex];
}

export const NEXT_BLOCK_ACCOUNTS = [
  new PublicKey('NextbLoCkVtMGcV47JzewQdvBpLqT9TxQFozQkN98pE'),
  new PublicKey('NexTbLoCkWykbLuB1NkjXgFWkX9oAtcoagQegygXXA2'),
  new PublicKey('NeXTBLoCKs9F1y5PJS9CKrFNNLU1keHW71rfh7KgA1X'),
  new PublicKey('NexTBLockJYZ7QD7p2byrUa6df8ndV2WSd8GkbWqfbb'),
  new PublicKey('neXtBLock1LeC67jYd1QdAa32kbVeubsfPNTJC1V5At'),
  new PublicKey('nEXTBLockYgngeRmRrjDV31mGSekVPqZoMGhQEZtPVG'),
  new PublicKey('NEXTbLoCkB51HpLBLojQfpyVAMorm3zzKg7w9NFdqid'),
  new PublicKey('nextBLoCkPMgmG8ZgJtABeScP35qLa2AMCNKntAP7Xc'),
];

export function getRandomNextBlockAccount() {
  const randomIndex = Math.floor(Math.random() * NEXT_BLOCK_ACCOUNTS.length);
  return NEXT_BLOCK_ACCOUNTS[randomIndex];
}

export const ASTRA_ACCOUNTS = [
  new PublicKey('astrazznxsGUhWShqgNtAdfrzP2G83DzcWVJDxwV9bF'),
  new PublicKey('astra4uejePWneqNaJKuFFA8oonqCE1sqF6b45kDMZm'),
  new PublicKey('astra9xWY93QyfG6yM8zwsKsRodscjQ2uU2HKNL5prk'),
  new PublicKey('astraRVUuTHjpwEVvNBeQEgwYx9w9CFyfxjYoobCZhL'),
];

export function getRandomAstraAccount() {
  const randomIndex = Math.floor(Math.random() * ASTRA_ACCOUNTS.length);
  return ASTRA_ACCOUNTS[randomIndex];
}
export const DEFAULT_BUY_AMOUNT = 0.5;
const BUY_AMOUNTS = [BigInt(DEFAULT_BUY_AMOUNT * LAMPORTS_PER_SOL)];

// const BUY_AMOUNTS = [
//   BigInt(1 * LAMPORTS_PER_SOL),
//   BigInt(1.2 * LAMPORTS_PER_SOL),
//   BigInt(1.3 * LAMPORTS_PER_SOL),
//   BigInt(1.5 * LAMPORTS_PER_SOL),
//   BigInt(0.35 * LAMPORTS_PER_SOL),
//   BigInt(0.8 * LAMPORTS_PER_SOL),
//   BigInt(0.425 * LAMPORTS_PER_SOL),
//   BigInt(0.75 * LAMPORTS_PER_SOL),
//   BigInt(1.2 * LAMPORTS_PER_SOL),
//   BigInt(0.8 * LAMPORTS_PER_SOL),
//   BigInt(0.4 * LAMPORTS_PER_SOL),
//   BigInt(1.5 * LAMPORTS_PER_SOL),
//   BigInt(1.08 * LAMPORTS_PER_SOL),
//   BigInt(0.6 * LAMPORTS_PER_SOL),
//   BigInt(0.56 * LAMPORTS_PER_SOL),
//   BigInt(0.84 * LAMPORTS_PER_SOL),
//   BigInt(0.55 * LAMPORTS_PER_SOL),
//   BigInt(1.02 * LAMPORTS_PER_SOL),
//   BigInt(0.75 * LAMPORTS_PER_SOL),
// ];

export function getRandomBuyAmmount() {
  const randomIndex = Math.floor(Math.random() * BUY_AMOUNTS.length);
  return BUY_AMOUNTS[randomIndex];
}

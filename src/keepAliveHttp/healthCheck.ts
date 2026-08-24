import { PublicKey } from '@solana/web3.js';
import { callUpstream } from '.';

const body = JSON.stringify({
  jsonrpc: '2.0',
  id: 1,
  method: 'getHealth',
});

const slotKey = process.env.SLOT_CONNECTION_KEY!;
const nextBlockKey = process.env.NEXTBLOCK_CONNECTION_KEY!;
const nodeKey = process.env.NODE_ONE_KEY!;
const astraKey = process.env.ASTRA_KEY!;

const jitoTipBody = JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'getTipAccounts', params: [] });

export const pingAstra = async () => {
  try {
    await callUpstream('astra', `/iris?api-key=${astraKey}`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Content-Length': Buffer.byteLength(body),
      },
      body: body,
    });
  } catch (e) {
    console.error(`astra issue:`, e);
  }
};

export const sendTransactionAstra = (tx: string) => {
  //create uuid
  const UUID = crypto.randomUUID();
  const txbody = JSON.stringify({
    jsonrpc: '2.0',
    id: UUID,
    method: 'sendTransaction',
    params: [tx, { encoding: 'base64', skipPreflight: true, maxRetries: 0 }],
  });
  callUpstream('astra', `/iris?api-key=${astraKey}`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(txbody),
    },
    body: txbody,
  }).catch(() => {});
};

export const pingSlot = async () => {
  try {
    await callUpstream('slot', `/?api-key=${slotKey}`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Content-Length': Buffer.byteLength(body),
      },
      body: body,
    });
  } catch (e) {
    console.error(`slot issue:`, e);
  }
};

export const sendTransactionSlot = (tx: string) => {
  //create uuid
  const UUID = crypto.randomUUID();
  const txbody = JSON.stringify({
    jsonrpc: '2.0',
    id: UUID,
    method: 'sendTransaction',
    params: [tx, { encoding: 'base64', skipPreflight: true, maxRetries: 0 }],
  });
  callUpstream('slot', `/?api-key=${slotKey}`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(txbody),
    },
    body: txbody,
  }).catch(() => {});
};

export const pingNextBlock = async () => {
  try {
    await callUpstream('nextBlock', '/api/v2/submit', {
      method: 'GET',
      headers: {
        Authorization: nextBlockKey,
      },
    });
  } catch (e) {
    // console.error(`nextBlock issue:`, e);
  }
};

export const sendTransactionNextBlock = (tx: string) => {
  const txbody = JSON.stringify({ transaction: { content: tx } });
  callUpstream('nextBlock', `/api/v2/submit`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(txbody),
      Authorization: nextBlockKey,
    },
    body: txbody,
  }).catch((e) => {
    console.error(`nextBlock issue:`, e);
  });
};

export const pingJito = async () => {
  try {
    await callUpstream('jito', '/api/v1/getTipAccounts', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Content-Length': Buffer.byteLength(jitoTipBody),
      },
      body: jitoTipBody,
    });
  } catch (e) {
    // keep-alive only
  }
};

/**
 * Jito used to be reached through the searcher SDK with an Anchor-built transaction. It is
 * now a plain provider: same prebuilt template, same byte patching, one keep-alive HTTPS
 * connection to the regional block engine.
 */
export const sendTransactionJito = (tx: string) => {
  const txbody = JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    method: 'sendTransaction',
    params: [tx, { encoding: 'base64', skipPreflight: true, maxRetries: 0 }],
  });
  callUpstream('jito', '/api/v1/transactions?bundleOnly=true', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(txbody),
    },
    body: txbody,
  }).catch(() => {
    // fire and forget: a provider error must never block the next launch
  });
};

export const pingNode = async () => {
  try {
    await callUpstream('node', '/ping', {
      method: 'GET',
      //   headers: {
      //     'api-key': nodeKey,
      //   },
    });
  } catch (e) {
    // console.error(`node issue:`, e);
  }
};

export const sendTransactionNode = (tx: string) => {
  //create uuid
  const UUID = crypto.randomUUID();
  const txbody = JSON.stringify({
    jsonrpc: '2.0',
    id: UUID,
    method: 'sendTransaction',
    params: [tx, { encoding: 'base64', skipPreflight: true, maxRetries: 0 }],
  });
  callUpstream('node', `/`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(txbody),
      'api-key': nodeKey,
    },
    body: txbody,
  }).catch(() => {});
};

/**
 * Helius Sender tip accounts. Sender routes across staked connections (Helius, Jito,
 * Harmonic, Rakurai) at once; the tip is what buys the priority buffer, and 0.001 SOL is the
 * documented minimum for it.
 */
export const HELIUS_SENDER_TIP_ACCOUNTS = [
  '4ACfpUFoaSD9bfPdeu6DBt89gB6ENTeHBXCAi87NhDEE',
  'D2L6yPZ2FmmmTKPgzaMKdhu6EWZcTpLy1Vhx8uvZe7NZ',
  '9bnz4RShgq1hAnLnZbP8kbgBg1kEmcJBYQq3gQbmnSta',
  '5VY91ws6B2hMmBFRsXkoAAdsPHBJwRfBht4DXox3xkwn',
  '2nyhqdwKcJZR2vcqCyrYsaPVdAnFoJjiksCXJ7hfEYgD',
  '2q5pghRs6arqVjRvT5gfgWfWcHWmw1ZuCzphgd5KfWGJ',
  'wyvPkWjVZz1M8fHQnMMCDTQDbkManefNNhweYk5WkcF',
  '3KCKozbAaF75qEU33jtzozcJ29yJuaLJTy2jFdzUY8bT',
  '4vieeGHPYPG2MmyPRcYjdiDmmhN3ww7hsFNap8pVN3Ey',
  '4TQLFNWK8AovT1gFvda5jfw2oJeRMKEmw7aH6MGBJ3or',
].map((a) => new PublicKey(a));

/** Sells go through Helius Sender rather than the plain RPC. No API key required. */
export const sendTransactionHeliusSender = (tx: string) => {
  const body = JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    method: 'sendTransaction',
    params: [tx, { encoding: 'base64', skipPreflight: true, maxRetries: 0 }],
  });
  return callUpstream('heliusSender', '/fast', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(body),
    },
    body,
  });
};

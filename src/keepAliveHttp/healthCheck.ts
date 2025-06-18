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
  }).catch((e) => {
    console.error(`astra issue:`, e);
  });
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
  }).catch((e) => {
    console.error(`slot issue:`, e);
  });
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
  }).catch((e) => {
    console.error(`node issue:`, e);
  });
};

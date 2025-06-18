import http, { Agent } from 'http';
import https from 'https';
import { HostKey } from './keepAliveHttp.types';
import { Region } from './keepAliveHttp.types';
import {
  SLOT_ENDPOINT_BY_REGION,
  ASTRA_ENDPOINT_BY_REGION,
  NODE1_ENDPOINT_BY_REGION,
  NEXTBLOCK_ENDPOINT_BY_REGION,
} from './keepAlive.consts';

const nodeRegion = process.env.NODE_REGION! as Region;
/** Configuration per upstream */
const CONFIG: Record<HostKey, { host: string; port: number }> = {
  slot: { host: SLOT_ENDPOINT_BY_REGION[nodeRegion], port: 80 }, // Assuming default HTTP port
  node: { host: NODE1_ENDPOINT_BY_REGION[nodeRegion]!, port: 80 },
  nextBlock: { host: NEXTBLOCK_ENDPOINT_BY_REGION[nodeRegion]!, port: 80 },
  astra: { host: ASTRA_ENDPOINT_BY_REGION[nodeRegion]!, port: 80 },
};

function wireAgentDebug(agent: Agent, key: string) {
  // called once per agent
  agent.on('free', (sock: any) => console.log(`[${key}] FREE   ${sock.localPort}`));
  agent.on('timeout', (sock: any) => console.log(`[${key}] TIMEOUT ${sock.localPort}`));
  agent.on('close', (sock: any) => console.log(`[${key}] CLOSE  ${sock.localPort}`));
}

/**
 * One-agent-per-host registry (true singleton because of Node's module cache).
 * Call get() with the desired key to obtain the long-lived Agent.
 */
class AgentRegistry {
  private static agents: Partial<Record<HostKey, http.Agent>> = {};

  /** Lazy-create & memoize */
  static get(key: HostKey): http.Agent {
    if (!this.agents[key]) {
      const config = CONFIG[key];
      const isHttps = config.port === 443;

      this.agents[key] = isHttps
        ? new https.Agent({
            keepAlive: true,
            keepAliveMsecs: 60_000,
            maxSockets: 6, // tune per host
            maxFreeSockets: 6,
          })
        : new http.Agent({
            keepAlive: true,
            keepAliveMsecs: 60_000,
            maxSockets: 6, // tune per host
            maxFreeSockets: 6,
          });
      // wireAgentDebug(this.agents[key]!, key);
    }
    return this.agents[key]!;
  }

  /** Handy accessor for host/port */
  static target(key: HostKey) {
    return CONFIG[key];
  }
}

export function callUpstream(
  key: HostKey,
  path: string,
  options: {
    method?: string;
    headers?: Record<string, string | number>;
    body?: string;
    timeout?: number;
  } = {},
): Promise<string> {
  return new Promise((resolve, reject) => {
    const { host, port } = AgentRegistry.target(key);
    const agent = AgentRegistry.get(key);
    const isHttps = port === 443;
    const requestOptions: http.RequestOptions = {
      hostname: host,
      path: path,
      port: port,
      method: options.method || 'GET',
      headers: options.headers || {},
      agent: agent,
      timeout: options.timeout || 5000, // 5 second timeout
    };
    const requestModule = isHttps ? https : http;
    const req = requestModule.request(requestOptions, (res) => {
      let data = '';
      res.on('data', (chunk) => {
        data += chunk;
      });

      res.on('end', () => {
        if (res.statusCode && res.statusCode >= 200 && res.statusCode < 300) {
          resolve(data);
        } else {
          reject(new Error(`HTTP ${res.statusCode}: ${data}`));
        }
      });
    });
    req.on('socket', (sock) => {
      sock.setNoDelay(true);
    });

    req.on('error', (err) => {
      reject(err);
    });

    req.on('timeout', () => {
      req.destroy();
      reject(new Error('Request timeout'));
    });

    // Write body if provided
    if (options.body) {
      req.write(options.body);
    }

    req.end();
  });
}

export default AgentRegistry;

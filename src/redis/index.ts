import { createClient, RedisClientType } from 'redis';
import { REDIS_WALLETS_KEY } from './redis.consts';

let redisClient: RedisClientType | null = null;

export async function getRedisClient(): Promise<RedisClientType> {
  if (!redisClient) {
    redisClient = createClient({
      url: 'redis://:Str0ngP1ss123123123123@localhost:6379',
    });

    redisClient.on('error', (err) => console.error('Redis error:', err));

    if (!redisClient.isOpen) {
      await redisClient.connect();
    }
  }

  return redisClient;
}

export async function sAdd(key: string, data: string): Promise<number> {
  const client = await getRedisClient();
  return await client.sAdd(key, data);
}

// set remove
export async function sRem(key: string, data: string): Promise<number> {
  const client = await getRedisClient();
  return await client.sRem(key, data);
}

export async function rPush(key: string, data: any): Promise<number> {
  const client = await getRedisClient();
  return await client.rPush(key, data);
}

export async function lRange(key: string): Promise<string[]> {
  const client = await getRedisClient();
  return await client.lRange(key, 0, -1);
}
export async function sMembers(key: string): Promise<string[]> {
  const client = await getRedisClient();
  return await client.sMembers(key);
}

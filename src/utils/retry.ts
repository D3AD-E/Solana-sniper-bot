export async function retryAsync<T>(fn: () => Promise<T>, retries: number = 3, delay: number = 50): Promise<T> {
  for (let attempt = 0; attempt <= retries; attempt++) {
    try {
      return await fn();
    } catch (error) {
      if (attempt === retries) {
        throw error;
      }
      await new Promise((resolve) => setTimeout(resolve, delay));
    }
  }
  // This line should never be reached
  throw new Error('Unexpected error in retryAsync');
}

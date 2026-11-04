/** Keep link/status checks from flooding forge and JIRA rate limits. */
export async function mapConcurrent<T, R>(
  items: readonly T[],
  map: (item: T) => Promise<R>,
  concurrency = 4,
): Promise<R[]> {
  const output: R[] = new Array(items.length);
  let cursor = 0;
  await Promise.all(
    Array.from({ length: Math.min(concurrency, items.length) }, async () => {
      while (cursor < items.length) {
        const index = cursor++;
        output[index] = await map(items[index]);
      }
    }),
  );
  return output;
}

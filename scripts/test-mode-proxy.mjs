/** Server-side backstop, in addition to the browser's mutation interceptor. */
export function protectTestProxy(proxy) {
  for (const [path, options] of Object.entries(proxy)) {
    if (!path.startsWith('/api/jira/') && !path.startsWith('/api/forges/')) continue;
    options.bypass = (request, response) => {
      if (request.method === 'GET') return undefined;
      if (!response) return false;
      response.statusCode = 405;
      response.setHeader('Allow', 'GET');
      response.setHeader('Content-Type', 'application/json');
      response.end(
        JSON.stringify({ error: 'Test mode: only read-only GET requests are allowed.' }),
      );
      // Vite stops processing a string bypass result when the response has ended.
      return request.url;
    };
  }
  return proxy;
}

# API Proxy Setup

Pullboard has no separate backend. Angular's development server forwards
same-origin API requests to the remote HTTPS APIs. Browser CORS restrictions
therefore do not apply between the dev server and the upstream API. This does
not disable remote authentication, TLS verification, or JIRA permissions.

```text
Browser at 127.0.0.1:4200
  /assets/config.json         -> local public/assets/config.json
  /api/forges/0/user           -> https://api.github.com/user
  /api/jira/rest/api/3/myself  -> https://your-team.atlassian.net/rest/api/3/myself
  /api/openai/v1/...          -> https://api.openai.com/v1/...
  /api/gemini/v1beta/...      -> https://generativelanguage.googleapis.com/v1beta/...
```

## Generate the proxy

`scripts/configure-proxy.mjs` reads the local config and writes ignored
`proxy.conf.json`. `npm start` and `npm run start:test` run it automatically. `angular.json` sets this
file as the `serve.options.proxyConfig`; it also binds to `127.0.0.1`.
`proxy.conf.example.json` is the tracked, credential-free example.

Each forge's array index maps to `/api/forges/<index>`. An enterprise target
ending in `/api/v3` retains that path when proxying relative API paths.
Reorder the forge array only while the server is stopped, then regenerate.

An actual rule in the generated JSON looks like:

```json
{
  "/api/jira/**": {
    "target": "https://your-team.atlassian.net",
    "secure": true,
    "changeOrigin": true,
    "headers": {
      "Origin": "https://your-team.atlassian.net",
      "Referer": "https://your-team.atlassian.net/"
    },
    "pathRewrite": { "^/api/jira": "" }
  }
}
```

`/**` includes nested paths with Angular's Vite-based dev server. `pathRewrite`
strips the local prefix; `changeOrigin` sets the upstream Host header;
the JIRA-only `headers` also align Origin and Referer with the upstream origin.
Without those overrides, browser writes can fail with `403 XSRF check failed`
even when authentication and read requests work. These headers are set by the
local proxy, not browser code, and do not change token or project permissions.
Restart `npm start` after proxy changes so the generated rules are reloaded.

`secure: true` keeps certificate verification enabled. Install a trusted
enterprise CA in the Node environment if necessary; do not turn off TLS checks.

The JSON contains hostnames but no credentials. Auth headers are supplied by
the individual service and forwarded only to that service's proxy route.
Angular's HTTP interceptor additionally refuses non-GET forge requests.

In test mode, `scripts/proxy.test-mode.mjs` loads those same generated targets
and adds a server-side read-only guard using `scripts/test-mode-proxy.mjs`.
Non-GET JIRA and forge requests receive HTTP 405 without being forwarded, even
if browser code bypasses Angular's interceptor. AI POST requests remain live.

## Direct ng serve

```sh
npm run configure-proxy
npx ng serve
# Or, to simulate all JIRA mutations:
npx ng serve --configuration test-mode
```

Generate before the first direct invocation on a clean checkout. The proxy is
not included in static production hosting; a built bundle alone cannot bypass
CORS. Pullboard is designed to run only through the local development server.

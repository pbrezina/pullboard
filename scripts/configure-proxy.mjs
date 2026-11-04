import { readFile, writeFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

function httpsUrl(value, label) {
  const url = new URL(value);
  if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash) {
    throw new Error(`${label} must be an HTTPS URL without credentials, query, or fragment.`);
  }
  return url.href.replace(/\/$/, '');
}

/** Vite's JSON proxy accepts http-proxy-middleware compatible pathRewrite rules. */
export function createProxy(config) {
  const route = (prefix, target) => ({
    target,
    secure: true,
    changeOrigin: true,
    pathRewrite: { [`^${prefix}`]: '' },
  });
  const jiraTarget = httpsUrl(config.jira.baseUrl, 'jira.baseUrl');
  const jiraOrigin = new URL(jiraTarget).origin;
  const proxy = {
    '/api/jira/**': {
      ...route('/api/jira', jiraTarget),
      // changeOrigin rewrites Host only. JIRA also checks browser Origin/Referer on writes.
      headers: { Origin: jiraOrigin, Referer: `${jiraOrigin}/` },
    },
    '/api/openai/**': route('/api/openai', 'https://api.openai.com'),
    '/api/gemini/**': route('/api/gemini', 'https://generativelanguage.googleapis.com'),
  };
  if (!Array.isArray(config.forges) || config.forges.length === 0) {
    throw new Error('Configure at least one forge.');
  }
  config.forges.forEach((forge, index) => {
    const prefix = `/api/forges/${index}`;
    proxy[`${prefix}/**`] = route(prefix, httpsUrl(forge.apiUrl, 'forge.apiUrl'));
  });
  return proxy;
}

export async function configureProxy() {
  let config;
  try {
    config = JSON.parse(await readFile('public/assets/config.json', 'utf8'));
  } catch {
    throw new Error('Create public/assets/config.json from config.example.json before starting.');
  }
  await writeFile('proxy.conf.json', `${JSON.stringify(createProxy(config), null, 2)}\n`, {
    mode: 0o600,
  });
  console.log(
    'Generated proxy.conf.json. Only use this app on loopback; never publish local credentials.',
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  configureProxy().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

import { readFileSync } from 'node:fs';
import { protectTestProxy } from './test-mode-proxy.mjs';

export default protectTestProxy(
  JSON.parse(readFileSync(new URL('../proxy.conf.json', import.meta.url), 'utf8')),
);

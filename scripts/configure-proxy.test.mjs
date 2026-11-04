import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { createProxy } from './configure-proxy.mjs';
import { protectTestProxy } from './test-mode-proxy.mjs';

const config = {
  jira: { baseUrl: 'https://team.atlassian.net' },
  forges: [
    { apiUrl: 'https://api.github.com', token: 'secret' },
    { apiUrl: 'https://git.example.com/api/v3/' },
  ],
};

test('routes every configured forge without embedding credentials', () => {
  const proxy = createProxy(config);
  assert.equal(proxy['/api/jira/**'].target, 'https://team.atlassian.net');
  assert.equal(proxy['/api/forges/1/**'].target, 'https://git.example.com/api/v3');
  assert.deepEqual(proxy['/api/forges/1/**'].pathRewrite, { '^/api/forges/1': '' });
  assert.equal(JSON.stringify(proxy).includes('secret'), false);
  for (const rule of Object.values(proxy)) {
    assert.equal(rule.secure, true);
    assert.equal(rule.changeOrigin, true);
  }
});

test('includes both supported LLM proxies', () => {
  const proxy = createProxy(config);
  assert.equal(proxy['/api/openai/**'].target, 'https://api.openai.com');
  assert.equal(proxy['/api/gemini/**'].target, 'https://generativelanguage.googleapis.com');
});

test('aligns JIRA browser origin headers with the upstream origin, only on the JIRA route', () => {
  for (const baseUrl of ['https://team.atlassian.net', 'https://jira.example.com:8443/jira/']) {
    // The header fix must survive serialization to the generated proxy.conf.json.
    const proxy = JSON.parse(JSON.stringify(createProxy({ ...config, jira: { baseUrl } })));
    const origin = new URL(baseUrl).origin;
    assert.deepEqual(proxy['/api/jira/**'].headers, {
      Origin: origin,
      Referer: `${origin}/`,
    });
    for (const [path, rule] of Object.entries(proxy)) {
      if (path !== '/api/jira/**') assert.equal(rule.headers, undefined);
    }
  }
});

test('the example proxy includes the same JIRA origin headers as the generator', async () => {
  const example = JSON.parse(
    await readFile(new URL('../proxy.conf.example.json', import.meta.url), 'utf8'),
  );
  const generated = createProxy({
    ...config,
    jira: { baseUrl: 'https://your-team.atlassian.net' },
  });
  assert.deepEqual(example['/api/jira/**'], generated['/api/jira/**']);
});

test('rejects insecure targets and credentials in URLs', () => {
  for (const baseUrl of [
    'http://team.atlassian.net',
    'https://user:secret@example.com',
    'https://example.com?key=secret',
  ]) {
    assert.throws(() => createProxy({ ...config, jira: { baseUrl } }));
  }
});

test('test mode proxy rejects every non-GET JIRA and forge request without forwarding', () => {
  const proxy = protectTestProxy(createProxy(config));
  assert.deepEqual(proxy['/api/jira/**'].headers, {
    Origin: 'https://team.atlassian.net',
    Referer: 'https://team.atlassian.net/',
  });
  for (const route of ['/api/jira/**', '/api/forges/0/**', '/api/forges/1/**']) {
    assert.equal(proxy[route].bypass({ method: 'GET', url: '/read' }, {}), undefined);
    for (const method of ['POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS']) {
      const headers = {};
      let body;
      const response = {
        setHeader: (key, value) => {
          headers[key] = value;
        },
        end: (value) => {
          body = value;
        },
      };
      assert.equal(proxy[route].bypass({ method, url: '/write' }, response), '/write');
      assert.equal(response.statusCode, 405);
      assert.equal(headers.Allow, 'GET');
      assert.match(body, /Test mode/);
    }
  }
  assert.equal(proxy['/api/openai/**'].bypass, undefined);
  assert.equal(proxy['/api/gemini/**'].bypass, undefined);
});

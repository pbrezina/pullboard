# Installation

## Prerequisites

- Node.js 24.15 or newer in the Node 24 LTS series and npm.
- Network access to your forge, JIRA Cloud, and chosen LLM API.
- A JIRA Software Scrum board and permission to read its issues and sprints.
  Normal-mode writes additionally need create, assign, link, schedule (when
  adding to a sprint), and transition permissions. An active sprint is optional.
- GitHub read-only credentials and an OpenAI or Gemini API key.

The repository already contains an Angular 22 standalone app, Angular Material,
Angular CDK, and Tailwind CSS 4. Do not create a second application or backend.

## Install and configure

From the repository root:

```sh
npm ci
cp public/assets/config.example.json public/assets/config.json
chmod 600 public/assets/config.json
```

Edit the new file using the [configuration reference](../user/configuration.md).
`public/assets/` is Angular's public assets directory; the browser reads the
file as `/assets/config.json`. Git ignores the credential-bearing file.

```sh
npm start
```

The prestart script generates `proxy.conf.json`, and Angular serves the app at
<http://127.0.0.1:4200>. There is no additional backend process.
For direct CLI usage, run `npm run configure-proxy` before `npx ng serve`.
Restart after modifying configuration, especially API targets or forge ordering.

For a safe first look, use `npm run start:test` instead of `npm start`. It reads
real data but simulates all JIRA changes. AI drafting remains live. See
[Test Mode](../user/test-mode.md) for details and the direct CLI equivalent.

**Do not use `--host 0.0.0.0`, expose a port forward, or deploy this application.**
The local assets contain API credentials. The development proxy is a local
transport convenience, not a credential vault or an authentication boundary.

## Verify the installation

```sh
npm run build
npm test -- --watch=false
npm run test:proxy
```

Production builds exclude the real config; development builds do not.
The API workflows require your own credentials for an end-to-end check.

## Build this book

```sh
cargo install mdbook --locked
mdbook build docs/
mdbook serve docs/ --hostname 127.0.0.1 --port 3000
```

Generated HTML is written to `docs/book/` (ignored). No diagram preprocessor
is necessary. Continue with [First Run](first-run.md).

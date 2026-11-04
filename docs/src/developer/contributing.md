# Contributing

Use the Angular CLI for scaffolding. Keep services cohesive and components
standalone; Angular 22 defaults to standalone and OnPush, so no redundant flags
are necessary. Prefer `inject()`, signals, Signal Forms and built-in control flow.

```sh
npm ci
npm run build
npm test -- --watch=false
npm run test:proxy
mdbook build docs/
```

Tests use Angular's Vitest builder and HTTP testing backend; no real credentials
or remote writes are needed. Never add secret-bearing fixtures. Test upstream
requests as GET-only and cover JIRA's non-transactional partial-failure path.
Test mode must never write through a newly added mutation path: cover service
simulation, the HTTP interceptor, and the server-side proxy guard. Use
`npm run start:test` for interactive checks with real reads but no JIRA writes;
AI drafting still uses your live provider. To validate the replaced environment,
run `npx ng build --configuration production,test-mode` as well.

Keep documentation in the mdBook under `docs/` and add every new chapter to
`docs/src/SUMMARY.md`.
Do not commit `docs/book/`, local config, or generated proxy output.

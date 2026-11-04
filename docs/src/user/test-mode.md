# Test Mode

Use test mode to explore the real dashboard and rehearse workflows without
changing any JIRA ticket or pull request:

```sh
npm run start:test
```

It generates the normal local API proxy settings, then starts Angular with the
`test-mode` serve configuration. The page displays a persistent
**TEST MODE — no changes saved** banner; the creation dialog also identifies
simulation explicitly. No change to `public/assets/config.json` is needed.

For the Angular CLI equivalent:

```sh
npm run configure-proxy
npx ng serve --configuration test-mode
```

Stop this server and run `npm start` (or plain `npx ng serve` after proxy setup)
to return to normal mode. Do not run normal and test-mode sessions side-by-side
when checking write safety. `npm test` is a separate, automated unit-test command.

## What happens

- Forge accounts, PRs, JIRA accounts, boards, sprints, tickets and Web Links are
  read from the real APIs. Filters, sprint selection and status checks work.
- Creating tickets, linking PRs, adding tickets to sprints, workflow transitions
  and ticket closure are simulated and reported inline. No remote mutations run.
- Simulated creations have no navigable fake JIRA link. Linking existing work
  can still show the real ticket link, clearly labelled as a simulated action.
- JIRA cards retain their real status after a simulated close. Refresh/reload
  continues to reflect the real server state, not an invented local board.
- Simulated results live only in memory. Normal-mode receipts are neither
  restored nor modified, and simulated receipts never appear in saved links or
  resume in normal mode. Reload to discard previews.
- AI suggestions **still use the real configured provider**, send PR title and
  description as usual, and may incur charges. Manual entry after an AI failure
  does not make further AI calls.

The Angular build replaces the environment flag; there is no runtime switch
that can disable it. Services simulate known mutations, an HTTP interceptor
rejects accidental non-GET JIRA requests, and the test-mode development proxy
rejects non-GET JIRA/forge requests with HTTP 405 before forwarding. LLM POST
requests remain permitted. Simulation is not a validation of write permissions,
create-screen requirements, or available workflow transitions in normal mode.

Test mode is not a credential sandbox. The same local credentials are loaded,
and reads still reach real systems. Keep the dev server on loopback and never
publish its assets or credential-bearing builds. Read-only remote tokens remain
recommended when rehearsing workflows.

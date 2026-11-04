# Configuration

Keep local credentials in `public/assets/config.json`, never in tracked source.
The canonical example is `public/assets/config.example.json` and the TypeScript
contract is `src/app/core/models.ts` (`AppConfig`, `ForgeConfig`, `JiraConfig`).

## Minimal payload

```json
{
  "forges": [
    {
      "type": "github",
      "token": "github-read-only-token",
      "apiUrl": "https://api.github.com"
    }
  ],
  "jira": {
    "email": "developer@example.com",
    "apiToken": "jira-api-token",
    "baseUrl": "https://your-team.atlassian.net",
    "projectKey": "DEV"
  },
  "llm": {
    "provider": "openai",
    "apiKey": "openai-api-key",
    "model": "gpt-4o-mini"
  }
}
```

`llm.provider` accepts `openai` or `gemini`. If omitted, **model** defaults to
`gpt-4o-mini` or `gemini-1.5-flash`, respectively. All other minimal fields are
required. These defaults are requested compatibility names, not guarantees of
current availability. In particular, Gemini 1.5 may no longer be served: choose
a model available to your account that supports JSON output. Provider errors
appear inline; no automatic fallback sends your data to another provider.

## Forge configuration

Each array entry is a separate account/instance. The token determines the
current user (`GET /user`). PRs are searched by that user's login.
`apiUrl` must be HTTPS. An enterprise entry can be:

```json
{
  "type": "github",
  "token": "enterprise-read-only-token",
  "apiUrl": "https://git.example.com/api/v3",
  "webUrl": "https://git.example.com"
}
```

Optional `webUrl` defines the trusted origin and base path for PR links. Public
GitHub defaults to `https://github.com`; enterprise defaults to the API origin.
Providers reject URLs outside that web origin/path; they never follow an
arbitrary JIRA Web Link with a forge Authorization header.

## JIRA discovery and overrides

By default Pullboard requires exactly one Scrum board for the project. It lists
active and future sprints and defaults to the unique active sprint, or to
**No sprint assigned** with a warning when no unique active sprint exists.
Creating without a sprint does not require an active sprint. It selects a
non-subtask Story issue type and a
numeric create-screen field named **Story Points** or **Story point estimate**.
Assignee must be available on the create screen. Other required fields need
project-side defaults.

On a shared board with several active sprints, configure a name template instead
of updating a sprint ID every time:

```json
{
  "sprintNamePattern": "%YEAR-IDM-SSSD-IDENTITY-S%NUMBER"
}
```

Place this option inside `jira`. `%YEAR` matches any four-digit year and
`%NUMBER` matches one or more digits, so this template matches
`2026-IDM-SSSD-IDENTITY-S23`. Matching is case-sensitive and covers the complete
name; all other characters are literal, not regular expressions. Unknown
placeholders are rejected. Active and future sprints on the selected board are
filtered across all result pages. Exactly one active sprint must match for an
automatic default; zero or multiple matches produce a warning and no default
sprint rather than selecting unrelated work or guessing the highest number.
You can explicitly select any listed sprint or view unsprinted tickets. This
continues to work across year changes.

Optional overrides inside `jira`:

```json
{
  "boardId": 12,
  "sprintId": 34,
  "issueTypeId": "10001",
  "storyPointsFieldId": "customfield_10016",
  "statuses": {
    "inProgress": ["In Progress"],
    "review": ["In Review", "Review"],
    "done": ["Done", "Closed"]
  }
}
```

Override examples are illustrative; discover IDs in your own tenant. `sprintId`
must name an active sprint on the selected board to provide the automatic
default and takes precedence over `sprintNamePattern`. That explicit sprint is
also included in the selector even when it does not match the pattern. An invalid
override warns without blocking unsprinted work. Without either setting, exactly
one active sprint provides the default. The viewing selector and creation
checkbox are independent; creating a ticket never adds it to a sprint unless
you opt in. Status arrays are ordered,
case-insensitive **destination status names**, not transition labels or IDs.
Default Review aliases are Review, In Review, and Code Review. A missing
transition is an error, never a guessed transition through intermediate states.

The configured email must resolve to the authenticated JIRA user. Pullboard
does not assign tickets to an unrelated account returned by a fuzzy search.

Restart `npm start` after edits. Configuration is cached in memory; proxy
targets are generated once when the development server starts.

Test mode is selected by `npm run start:test` or
`npx ng serve --configuration test-mode`, not by a configuration field. See
[Test Mode](test-mode.md).

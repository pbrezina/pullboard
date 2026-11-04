# Forge Providers

The TypeScript contracts are in `src/app/core/models.ts`:

```ts
interface IForgeProvider {
  readonly id: string;
  readonly label: string;
  getUsername(): Promise<string>;
  supportsUrl(url: string): boolean;
  getOpenPullRequests(): Promise<UnifiedPullRequest[]>;
  getPullRequestInbox(): Promise<PullRequestInbox>;
  getPullRequestStatus(url: string): Promise<PullRequestStatus>;
}
```

`UnifiedPullRequest` includes stable ID, provider ID, forge label, number,
title, description, canonical web URL, repository, author, reviewer and assignee
arrays, label names, optional creation timestamp (`createdAt`), update timestamp,
state (`open`, `closed`, `merged`), and draft flag. Never expose a forge-specific
response type to a component. Preserve merged versus unmerged-closed states.
For GitHub, bounded detail requests supply assignees and requested reviewers/teams;
paginated reviews add past reviewers without duplicates. Exclude the PR author
from reviewers, since GitHub represents their replies as review records too.
Sort reviewers and assignees alphabetically, ignoring case, including partial
results after a lookup failure. If participant lookup
fails, return the PR with known participants and `participantsError`, rather than
dropping the entire PR list or claiming missing metadata is empty.

`getOpenPullRequests()` remains authored-only for JIRA syncing. The independent
`getPullRequestInbox()` returns `{ opened, requested, warnings }`, with normalized
`PullRequestWorkItem` entries. These add `reviewStatus` (`outstanding`/`given`),
`reviewRequestedAt`, `mentionedAt`, `needsAction` and optional `attentionError`.
`ForgeAggregatorService` preserves successful accounts and adds identities.
GitHub unions author, review-requested, reviewed-by and mentions searches,
deduplicates candidates, and enriches them with paginated reviews, conversation
comments, inline comments and timeline events. Classification is in
`github-activity.ts`; UI sorting and label helpers are in `pull-request-order.ts`.
Provider implementations must report incomplete activity rather than silently
claiming it is empty. Keep all operations GET-only.

## Add another forge

1. Scaffold a provider with `npx ng generate service core/gitlab-provider`.
2. Implement `IForgeProvider` with an instance per config entry. Like
   `GithubProviderService`, it may be a plain strategy constructed with an
   injected `HttpClient`, config and array index rather than a root singleton.
3. Use **GET only** through `/api/forges/${index}`. The proxy generator already
   creates an index route for every forge regardless of type. Use least-privilege
   read-only tokens; never add comment, label, state-change, or merge methods.
4. Expose the authenticated user's login via `getUsername()` for the header and
   PR enumeration; cache the lookup per provider instance. Paginate,
   normalize draft flags, and preserve the correct repository and author.
5. Implement strict URL recognition: verify the configured web origin and path,
   parse only the provider's PR URL shape, then construct the relative API
   endpoint yourself. Never send an Authorization header to an arbitrary link.
6. Register the factory in `FORGE_PROVIDER_FACTORIES` in
   `core/forge-aggregator.ts`, preserving the existing GitHub entry:

```ts
return [
  { type: 'github', create: (config, index) => new GithubProviderService(http, config, index) },
  { type: 'gitlab', create: (config, index) => new GitlabProviderService(http, config, index) },
];
```

7. Add a `forges` config entry with the registry type, read-only token,
   HTTPS `apiUrl`, and `webUrl` if needed. Restart `npm start`.
8. Test pagination, multiple accounts, authentication, invalid/malicious URLs,
   drafts, open/closed/merged states, and an all-GET HTTP surface.

No dashboard changes are necessary: forge badges use the normalized label,
and closure monitoring uses `supportsUrl` and `getPullRequestStatus`.
`GitlabProviderService` above is an extension example, not a bundled provider.

When accounts share a forge origin, Web Link status checks try matching
configured accounts in order on access-denied/not-found responses. Rate limits
and connectivity failures are not retried across accounts. Direct sync actions
include the PR's provider ID so they use the authoring account.

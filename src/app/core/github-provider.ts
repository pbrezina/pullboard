import { HttpClient } from '@angular/common/http';
import { firstValueFrom } from 'rxjs';
import { mapConcurrent } from './concurrency';
import { errorMessage } from './errors';
import {
  ForgeConfig,
  IForgeProvider,
  PullRequestInbox,
  PullRequestStatus,
  PullRequestWorkItem,
  UnifiedPullRequest,
} from './models';
import { analyzeGithubActivity, GithubComment, GithubEvent, GithubReview } from './github-activity';
import { sortedLabels } from './pull-request-order';

interface GithubPullRequest {
  number: number;
  title: string;
  body: string | null;
  html_url: string;
  draft?: boolean;
  state: string;
  merged_at?: string | null;
  user: { login: string };
  assignees?: { login: string }[];
  requested_reviewers?: { login: string }[];
  requested_teams?: { slug: string }[];
  updated_at: string;
  created_at?: string;
  labels?: { name: string }[];
}

const participantOrder = new Intl.Collator('en', { sensitivity: 'base' });

/** One instance per configured account. HTTP surface is intentionally GET-only. */
export class GithubProviderService implements IForgeProvider {
  readonly label = 'GitHub';
  readonly id: string;
  private readonly base: string;
  private readonly web: URL;
  private readonly headers: Record<string, string>;
  private user?: Promise<string>;

  constructor(
    private readonly http: HttpClient,
    config: ForgeConfig,
    index: number,
  ) {
    this.id = `forge-${index}`;
    this.base = `/api/forges/${index}`;
    this.web = new URL(
      config.webUrl ??
        (new URL(config.apiUrl).hostname === 'api.github.com'
          ? 'https://github.com'
          : new URL(config.apiUrl).origin),
    );
    this.headers = {
      Authorization: `Bearer ${config.token}`,
      Accept: 'application/vnd.github+json',
      'X-GitHub-Api-Version': '2022-11-28',
    };
  }

  private parse(url: string): { owner: string; repo: string; number: string } | undefined {
    try {
      const candidate = new URL(url);
      const prefix = this.web.pathname.replace(/\/$/, '');
      if (
        candidate.origin !== this.web.origin ||
        candidate.username ||
        candidate.password ||
        !candidate.pathname.startsWith(`${prefix}/`)
      )
        return undefined;
      const match = /^\/([\w.-]+)\/([\w.-]+)\/pull\/([1-9]\d*)\/?$/.exec(
        candidate.pathname.slice(prefix.length),
      );
      return match ? { owner: match[1], repo: match[2], number: match[3] } : undefined;
    } catch {
      return undefined;
    }
  }

  supportsUrl(url: string): boolean {
    return !!this.parse(url);
  }

  getUsername(): Promise<string> {
    return (this.user ??= firstValueFrom(
      this.http.get<{ login: string }>(`${this.base}/user`, { headers: this.headers }),
    )
      .then((user) => user.login)
      .catch((error: unknown) => {
        this.user = undefined;
        throw error;
      }));
  }

  async getOpenPullRequests(): Promise<UnifiedPullRequest[]> {
    const author = await this.getUsername();
    const result = await this.search(`author:${author}`);
    return mapConcurrent(result, (pr) => this.withParticipants(pr));
  }

  private async search(qualifier: string): Promise<UnifiedPullRequest[]> {
    const result: UnifiedPullRequest[] = [];
    for (let page = 1; ; page++) {
      const response = await firstValueFrom(
        this.http.get<{
          items: GithubPullRequest[];
          total_count: number;
          incomplete_results: boolean;
        }>(`${this.base}/search/issues`, {
          headers: this.headers,
          params: {
            q: `is:pr is:open ${qualifier} archived:false`,
            sort: 'updated',
            order: 'desc',
            per_page: 100,
            page,
          },
        }),
      );
      if (response.incomplete_results || response.total_count > 1000)
        throw new Error(
          'GitHub search is incomplete or exceeds 1,000 PRs. Narrow the account scope before syncing.',
        );
      for (const pr of response.items) {
        const location = this.parse(pr.html_url);
        if (!location) continue;
        result.push({
          id: `${this.id}:${location.owner}/${location.repo}#${pr.number}`,
          providerId: this.id,
          forge: this.label,
          number: pr.number,
          title: pr.title,
          description: pr.body ?? '',
          url: pr.html_url,
          repository: `${location.owner}/${location.repo}`,
          author: pr.user.login,
          reviewers: [],
          assignees: [],
          labels: sortedLabels((pr.labels ?? []).map((label) => label.name)),
          state: 'open',
          draft: pr.draft ?? false,
          createdAt: pr.created_at,
          updatedAt: pr.updated_at,
        });
      }
      if (page * 100 >= response.total_count || response.items.length === 0) break;
    }
    return result;
  }

  async getPullRequestInbox(): Promise<PullRequestInbox> {
    const username = await this.getUsername();
    const qualifiers = [
      `author:${username}`,
      `review-requested:${username}`,
      `reviewed-by:${username}`,
      `mentions:${username}`,
    ];
    const searches = await Promise.allSettled(
      qualifiers.map((qualifier) => this.search(qualifier)),
    );
    const warnings: string[] = [];
    const candidates = new Map<
      string,
      { pr: UnifiedPullRequest; requested: boolean; reviewed: boolean }
    >();
    searches.forEach((result, index) => {
      if (result.status === 'rejected') {
        warnings.push(
          `${qualifiers[index]}: ${errorMessage(result.reason)}. This part of the list is incomplete.`,
        );
        return;
      }
      for (const pr of result.value) {
        const candidate = candidates.get(pr.id) ?? { pr, requested: false, reviewed: false };
        if (index === 1) candidate.requested = true;
        if (index === 2) candidate.reviewed = true;
        candidates.set(pr.id, candidate);
      }
    });
    const items = await mapConcurrent([...candidates.values()], async (candidate) =>
      this.withActivity(candidate.pr, username, candidate.requested, candidate.reviewed),
    );
    return {
      opened: items.filter(
        (pr) => pr.state === 'open' && pr.author.toLowerCase() === username.toLowerCase(),
      ),
      requested: items.filter(
        (pr) =>
          pr.state === 'open' &&
          (!!pr.mentionedAt ||
            (pr.author.toLowerCase() !== username.toLowerCase() &&
              (!!pr.reviewStatus || !!pr.attentionError))),
      ),
      warnings,
    };
  }

  private async pages<T>(path: string): Promise<T[]> {
    const items: T[] = [];
    for (let page = 1; ; page++) {
      const batch = await firstValueFrom(
        this.http.get<T[]>(path, {
          headers: this.headers,
          params: { per_page: 100, page },
        }),
      );
      items.push(...batch);
      if (batch.length < 100) return items;
    }
  }

  private async withActivity(
    pr: UnifiedPullRequest,
    username: string,
    requested: boolean,
    reviewed: boolean,
  ): Promise<PullRequestWorkItem> {
    const location = this.parse(pr.url)!;
    const repo = `${this.base}/repos/${location.owner}/${location.repo}`;
    const path = `${repo}/pulls/${pr.number}`;
    const results = await Promise.allSettled([
      firstValueFrom(this.http.get<GithubPullRequest>(path, { headers: this.headers })),
      this.pages<GithubReview>(`${path}/reviews`),
      this.pages<GithubComment>(`${repo}/issues/${pr.number}/comments`),
      this.pages<GithubComment>(`${path}/comments`),
      this.pages<GithubEvent>(`${repo}/issues/${pr.number}/timeline`),
    ] as const);
    const [details, reviews, comments, reviewComments, timeline] = results;
    const errors = results.flatMap((result, index) =>
      result.status === 'rejected'
        ? [
            `${['PR details', 'reviews', 'conversation', 'review threads', 'timeline'][index]}: ${errorMessage(result.reason)}`,
          ]
        : [],
    );
    const reviewData = reviews.status === 'fulfilled' ? reviews.value : [];
    const detail = details.status === 'fulfilled' ? details.value : undefined;
    const directlyRequested = detail?.requested_reviewers?.some(
      (user) => user.login.toLowerCase() === username.toLowerCase(),
    );
    const reviewers = new Set([
      ...(detail?.requested_reviewers ?? []).map((user) => user.login),
      ...(detail?.requested_teams ?? []).map((team) => `${location.owner}/${team.slug} (team)`),
      ...reviewData.flatMap((review) => (review.user ? [review.user.login] : [])),
    ]);
    const item = analyzeGithubActivity(
      {
        ...pr,
        title: detail?.title ?? pr.title,
        description: detail?.body === null ? '' : (detail?.body ?? pr.description),
        draft: detail?.draft ?? pr.draft,
        state: detail?.merged_at ? 'merged' : detail?.state === 'closed' ? 'closed' : pr.state,
        updatedAt: detail?.updated_at ?? pr.updatedAt,
        createdAt: detail?.created_at ?? pr.createdAt,
        labels: sortedLabels(
          (detail?.labels ?? pr.labels.map((name) => ({ name }))).map((label) => label.name),
        ),
        reviewers: [...reviewers]
          .filter((user) => user.toLowerCase() !== pr.author.toLowerCase())
          .sort(participantOrder.compare),
        assignees: (detail?.assignees ?? [])
          .map((user) => user.login)
          .sort(participantOrder.compare),
      },
      username,
      {
        requested: detail?.requested_reviewers
          ? !!directlyRequested || (requested && !!detail.requested_teams?.length)
          : requested,
        directlyRequested,
        reviewed,
        createdAt: detail?.created_at ?? pr.createdAt,
        reviews: reviewData,
        comments: comments.status === 'fulfilled' ? comments.value : [],
        reviewComments: reviewComments.status === 'fulfilled' ? reviewComments.value : [],
        timeline: timeline.status === 'fulfilled' ? timeline.value : [],
      },
    );
    return {
      ...item,
      ...(errors.length
        ? {
            attentionError: `Activity is incomplete; badges and ordering may be stale. ${errors.join(' ')}`,
          }
        : {}),
    };
  }

  private async withParticipants(pr: UnifiedPullRequest): Promise<UnifiedPullRequest> {
    const location = this.parse(pr.url)!;
    const path = `${this.base}/repos/${location.owner}/${location.repo}/pulls/${pr.number}`;
    const reviewers = new Set<string>();
    let assignees: string[] = [];
    let participantsError: string | undefined;
    let createdAt = pr.createdAt;
    try {
      const details = await firstValueFrom(
        this.http.get<GithubPullRequest>(path, { headers: this.headers }),
      );
      assignees = (details.assignees ?? []).map((user) => user.login);
      createdAt = details.created_at ?? createdAt;
      for (const user of details.requested_reviewers ?? []) reviewers.add(user.login);
      for (const team of details.requested_teams ?? [])
        reviewers.add(`${location.owner}/${team.slug} (team)`);
      // Include people who already reviewed, not only outstanding review requests.
      for (let page = 1; ; page++) {
        const reviews = await firstValueFrom(
          this.http.get<{ user: { login: string } | null }[]>(`${path}/reviews`, {
            headers: this.headers,
            params: { per_page: 100, page },
          }),
        );
        for (const review of reviews) if (review.user) reviewers.add(review.user.login);
        if (reviews.length < 100) break;
      }
    } catch (error) {
      // A failed metadata lookup must not hide an otherwise usable PR.
      participantsError = `Could not load all reviewers and assignees. ${errorMessage(error)}`;
    }
    return {
      ...pr,
      createdAt,
      // GitHub also returns the author's replies as COMMENTED reviews, not reviewer assignments.
      reviewers: [...reviewers]
        .filter((login) => login.toLowerCase() !== pr.author.toLowerCase())
        .sort(participantOrder.compare),
      assignees: assignees.sort(participantOrder.compare),
      ...(participantsError ? { participantsError } : {}),
    };
  }

  async getPullRequestStatus(url: string): Promise<PullRequestStatus> {
    const pr = this.parse(url);
    if (!pr) throw new Error('PR URL is not supported by this GitHub configuration.');
    const data = await firstValueFrom(
      this.http.get<GithubPullRequest>(
        `${this.base}/repos/${pr.owner}/${pr.repo}/pulls/${pr.number}`,
        { headers: this.headers },
      ),
    );
    return {
      state: data.merged_at ? 'merged' : data.state === 'closed' ? 'closed' : 'open',
      draft: data.draft ?? false,
    };
  }
}

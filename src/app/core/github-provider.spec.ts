import { HttpClient, provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { GithubProviderService } from './github-provider';
import { testConfig, testPr } from './testing';

describe('GithubProviderService', () => {
  let http: HttpTestingController;
  let provider: GithubProviderService;
  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting()],
    });
    http = TestBed.inject(HttpTestingController);
    provider = new GithubProviderService(TestBed.inject(HttpClient), testConfig.forges[0], 0);
  });
  afterEach(() => http.verify());

  it('resolves the current user and normalizes authored draft PRs with GET only', async () => {
    const pending = provider.getOpenPullRequests();
    const user = http.expectOne('/api/forges/0/user');
    expect(user.request.method).toBe('GET');
    expect(user.request.headers.get('Authorization')).toBe('Bearer test-read-only');
    user.flush({ login: 'dev' });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    const search = http.expectOne((request) => request.url.endsWith('/search/issues'));
    expect(search.request.method).toBe('GET');
    expect(search.request.params.get('q')).toBe('is:pr is:open author:dev archived:false');
    search.flush({
      items: [
        {
          number: 42,
          title: testPr.title,
          body: null,
          html_url: testPr.url,
          draft: true,
          user: { login: 'dev' },
          updated_at: testPr.updatedAt,
          created_at: '2026-10-01T10:00:00Z',
        },
      ],
      total_count: 1,
      incomplete_results: false,
    });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    const details = http.expectOne('/api/forges/0/repos/org/repo/pulls/42');
    expect(details.request.method).toBe('GET');
    details.flush({
      assignees: [{ login: 'owner' }, { login: 'co-owner' }],
      requested_reviewers: [{ login: 'reviewer' }],
      requested_teams: [{ slug: 'maintainers' }],
    });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    const reviews = http.expectOne((request) => request.url.endsWith('/pulls/42/reviews'));
    expect(reviews.request.method).toBe('GET');
    reviews.flush([
      { user: { login: 'reviewer' } },
      { user: { login: 'already-reviewed' } },
      { user: { login: 'already-reviewed' } },
      { user: null },
    ]);
    expect(await pending).toEqual([
      {
        ...testPr,
        description: '',
        createdAt: '2026-10-01T10:00:00Z',
        draft: true,
        reviewers: ['already-reviewed', 'org/maintainers (team)', 'reviewer'],
        assignees: ['co-owner', 'owner'],
      },
    ]);
  });

  it('does not count the PR author’s review replies as a reviewer', async () => {
    const pending = provider.getOpenPullRequests();
    http.expectOne('/api/forges/0/user').flush({ login: 'pbrezina' });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    http
      .expectOne((request) => request.url.endsWith('/search/issues'))
      .flush({
        items: [
          {
            number: 8982,
            title: 'failover: implement offline detection',
            html_url: 'https://github.com/SSSD/sssd/pull/8982',
            user: { login: 'pbrezina' },
            updated_at: testPr.updatedAt,
          },
        ],
        total_count: 1,
        incomplete_results: false,
      });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    http.expectOne('/api/forges/0/repos/SSSD/sssd/pulls/8982').flush({
      assignees: [{ login: 'justin-stephenson' }, { login: 'alexey-tikhonov' }],
      requested_reviewers: [{ login: 'alexey-tikhonov' }],
    });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    http
      .expectOne((request) => request.url.endsWith('/pulls/8982/reviews'))
      .flush([
        { user: { login: 'gemini-code-assist[bot]' }, state: 'COMMENTED' },
        { user: { login: 'justin-stephenson' }, state: 'COMMENTED' },
        { user: { login: 'pbrezina' }, state: 'COMMENTED' },
        { user: { login: 'PBrezina' }, state: 'COMMENTED' },
        { user: { login: 'justin-stephenson' }, state: 'APPROVED' },
      ]);
    const [pr] = await pending;
    expect(pr.reviewers).toEqual([
      'alexey-tikhonov',
      'gemini-code-assist[bot]',
      'justin-stephenson',
    ]);
    expect(pr.assignees).toEqual(['alexey-tikhonov', 'justin-stephenson']);
  });

  it.each([
    ['open', null, 'open'],
    ['closed', null, 'closed'],
    ['closed', '2026-11-01', 'merged'],
  ])('maps remote state %s / merged_at %s to %s', async (state, merged_at, expected) => {
    const pending = provider.getPullRequestStatus(testPr.url);
    const request = http.expectOne('/api/forges/0/repos/org/repo/pulls/42');
    expect(request.request.method).toBe('GET');
    request.flush({ state, merged_at, draft: false });
    expect(await pending).toEqual({ state: expected, draft: false });
  });

  it('never forwards credentials to arbitrary URLs or GitHub issues', async () => {
    for (const url of [
      'https://evil.example/org/repo/pull/42',
      'https://github.com.evil.example/org/repo/pull/42',
      'https://github.com/org/repo/issues/42',
      'https://secret@github.com/org/repo/pull/42',
    ]) {
      expect(provider.supportsUrl(url)).toBe(false);
      await expect(provider.getPullRequestStatus(url)).rejects.toThrow();
    }
    http.expectNone(() => true);
  });

  it('supports enterprise origins and per-account proxy indices', async () => {
    provider = new GithubProviderService(
      TestBed.inject(HttpClient),
      { ...testConfig.forges[0], apiUrl: 'https://git.example.com/api/v3' },
      2,
    );
    expect(provider.supportsUrl('https://git.example.com/org/repo/pull/42')).toBe(true);
    const pending = provider.getPullRequestStatus('https://git.example.com/org/repo/pull/42');
    http.expectOne('/api/forges/2/repos/org/repo/pulls/42').flush({ state: 'open', draft: true });
    expect(await pending).toEqual({ state: 'open', draft: true });
  });

  it('refuses incomplete or silently truncated search results', async () => {
    const pending = provider.getOpenPullRequests();
    const assertion = expect(pending).rejects.toThrow('incomplete');
    http.expectOne('/api/forges/0/user').flush({ login: 'dev' });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    http
      .expectOne((request) => request.url.endsWith('/search/issues'))
      .flush({ items: [], total_count: 1001, incomplete_results: false });
    await assertion;
  });

  it('paginates authored PRs instead of silently returning only the first page', async () => {
    const pending = provider.getOpenPullRequests();
    http.expectOne('/api/forges/0/user').flush({ login: 'dev' });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    const first = http.expectOne((request) => request.url.endsWith('/search/issues'));
    const item = {
      number: 42,
      title: testPr.title,
      body: '',
      html_url: testPr.url,
      draft: false,
      user: { login: 'dev' },
      updated_at: testPr.updatedAt,
    };
    first.flush({ items: [item], total_count: 101, incomplete_results: false });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    const second = http.expectOne((request) => request.url.endsWith('/search/issues'));
    expect(second.request.params.get('page')).toBe('2');
    second.flush({
      items: [{ ...item, number: 43, html_url: 'https://github.com/org/repo/pull/43' }],
      total_count: 101,
      incomplete_results: false,
    });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    for (const number of [42, 43])
      http.expectOne(`/api/forges/0/repos/org/repo/pulls/${number}`).flush({});
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    for (const number of [42, 43])
      http.expectOne((request) => request.url.endsWith(`/pulls/${number}/reviews`)).flush([]);
    expect((await pending).map((pr) => pr.number)).toEqual([42, 43]);
  });

  it('keeps PRs visible and reports unavailable participants when enrichment fails', async () => {
    const pending = provider.getOpenPullRequests();
    http.expectOne('/api/forges/0/user').flush({ login: 'dev' });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    http
      .expectOne((request) => request.url.endsWith('/search/issues'))
      .flush({
        items: [
          { ...testPr, html_url: testPr.url, user: { login: 'dev' }, updated_at: testPr.updatedAt },
        ],
        total_count: 1,
        incomplete_results: false,
      });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    http
      .expectOne('/api/forges/0/repos/org/repo/pulls/42')
      .flush({}, { status: 403, statusText: 'Forbidden' });
    const [pr] = await pending;
    expect(pr.url).toBe(testPr.url);
    expect(pr.reviewers).toEqual([]);
    expect(pr.assignees).toEqual([]);
    expect(pr.participantsError).toContain('Could not load');
  });

  it('paginates completed reviews and retains known participants on a later failure', async () => {
    const pending = provider.getOpenPullRequests();
    http.expectOne('/api/forges/0/user').flush({ login: 'dev' });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    http
      .expectOne((request) => request.url.endsWith('/search/issues'))
      .flush({
        items: [
          { ...testPr, html_url: testPr.url, user: { login: 'dev' }, updated_at: testPr.updatedAt },
        ],
        total_count: 1,
        incomplete_results: false,
      });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    http.expectOne('/api/forges/0/repos/org/repo/pulls/42').flush({
      assignees: [{ login: 'Zulu' }, { login: 'alpha' }],
      requested_reviewers: [{ login: 'zoe' }],
    });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    http
      .expectOne((request) => request.url.endsWith('/reviews'))
      .flush(
        Array.from({ length: 100 }, (_, index) => ({ user: { login: index ? 'Alice' : 'DEV' } })),
      );
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    const second = http.expectOne((request) => request.url.endsWith('/reviews'));
    expect(second.request.params.get('page')).toBe('2');
    second.flush({}, { status: 503, statusText: 'Unavailable' });
    const [pr] = await pending;
    expect(pr.reviewers).toEqual(['Alice', 'zoe']);
    expect(pr.assignees).toEqual(['alpha', 'Zulu']);
    expect(pr.participantsError).toContain('Could not load');
  });

  describe('pull request inbox', () => {
    const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));
    const item = (number: number, author = 'colleague') => ({
      number,
      title: `PR ${number}`,
      body: '',
      html_url: `https://github.com/org/repo/pull/${number}`,
      user: { login: author },
      created_at: '2026-10-01T10:00:00Z',
      updated_at: testPr.updatedAt,
      draft: false,
      labels: [{ name: 'Zulu' }, { name: 'Accepted' }, { name: 'alpha' }],
    });
    async function load(
      searches: Record<string, ReturnType<typeof item>[]>,
      respond: (path: string) => unknown = () => [],
      failSearch?: string,
    ) {
      const pending = provider.getPullRequestInbox();
      http.expectOne('/api/forges/0/user').flush({ login: 'dev' });
      await tick();
      const requests = http.match((request) => request.url.endsWith('/search/issues'));
      expect(requests).toHaveLength(4);
      for (const request of requests) {
        expect(request.request.method).toBe('GET');
        const query = request.request.params.get('q')!;
        const qualifier = query.split(' ')[2];
        expect(query).toBe(`is:pr is:open ${qualifier} archived:false`);
        if (qualifier === failSearch)
          request.flush({}, { status: 422, statusText: 'Search unavailable' });
        else {
          const items = searches[qualifier] ?? [];
          request.flush({ items, total_count: items.length, incomplete_results: false });
        }
      }
      await tick();
      for (const request of http.match(() => true)) {
        expect(request.request.method).toBe('GET');
        if (request.request.url.endsWith('/pulls/99')) {
          request.flush({}, { status: 403, statusText: 'Forbidden' });
        } else {
          request.flush(respond(request.request.url) as object);
        }
      }
      return pending;
    }

    it('unions authored, requested, reviewed and mentioned searches without duplicate cards', async () => {
      const result = await load(
        {
          'author:dev': [item(1, 'dev')],
          'review-requested:dev': [item(2)],
          'reviewed-by:dev': [item(2), item(3)],
          'mentions:dev': [item(2)],
        },
        (path) => {
          if (path.endsWith('/pulls/2'))
            return { requested_reviewers: [{ login: 'dev' }], assignees: [] };
          if (path.endsWith('/issues/2/comments'))
            return [
              { id: 1, user: { login: 'colleague' }, body: '@dev?', created_at: testPr.updatedAt },
            ];
          if (path.endsWith('/issues/2/timeline'))
            return [
              {
                event: 'review_requested',
                requested_reviewer: { login: 'dev' },
                created_at: testPr.updatedAt,
              },
            ];
          return [];
        },
      );
      expect(result.opened.map((pr) => pr.number)).toEqual([1]);
      expect(result.opened[0].createdAt).toBe('2026-10-01T10:00:00Z');
      expect(result.requested.map((pr) => pr.number)).toEqual([2, 3]);
      expect(result.requested[0].reviewStatus).toBe('outstanding');
      expect(result.requested[0].mentionedAt).toBe(testPr.updatedAt);
      expect(result.requested[0].reviewRequestedAt).toBe(testPr.updatedAt);
      expect(result.requested[0].labels).toEqual(['Accepted', 'alpha', 'Zulu']);
      expect(result.requested[1].reviewStatus).toBe('given');
    });

    it('retains a team review request matched by GitHub search', async () => {
      const result = await load({ 'review-requested:dev': [item(2)] }, (path) =>
        path.endsWith('/pulls/2')
          ? { requested_reviewers: [], requested_teams: [{ slug: 'maintainers' }] }
          : [],
      );
      expect(result.requested[0].reviewStatus).toBe('outstanding');
      expect(result.requested[0].reviewers).toEqual(['org/maintainers (team)']);
      expect(result.requested[0].reviewRequestedAt).toBeUndefined();
    });

    it('includes unanswered mentions on your own PRs in both relevant columns', async () => {
      const result = await load(
        { 'author:dev': [item(1, 'dev')], 'mentions:dev': [item(1, 'dev')] },
        (path) =>
          path.endsWith('/issues/1/comments')
            ? [
                {
                  id: 1,
                  user: { login: 'colleague' },
                  body: '@dev please reply',
                  created_at: testPr.updatedAt,
                },
              ]
            : [],
      );
      expect(result.opened.map((pr) => pr.number)).toEqual([1]);
      expect(result.requested.map((pr) => pr.number)).toEqual([1]);
    });

    it('hides answered mention-only PRs, but not already-reviewed PRs', async () => {
      const result = await load(
        { 'mentions:dev': [item(2)], 'reviewed-by:dev': [item(3)] },
        (path) =>
          path.endsWith('/issues/2/comments')
            ? [
                {
                  id: 1,
                  user: { login: 'colleague' },
                  body: '@dev?',
                  created_at: '2026-11-01T10:00:00Z',
                },
                {
                  id: 2,
                  user: { login: 'dev' },
                  body: 'Answered',
                  created_at: '2026-11-02T10:00:00Z',
                },
              ]
            : [],
      );
      expect(result.requested.map((pr) => pr.number)).toEqual([3]);
    });

    it('keeps cards with warnings when details are inaccessible', async () => {
      const result = await load({ 'review-requested:dev': [item(99)] });
      expect(result.requested[0].reviewStatus).toBe('outstanding');
      expect(result.requested[0].attentionError).toContain('PR details');
    });

    it('retains successful search categories and reports failed ones', async () => {
      const result = await load({ 'author:dev': [item(1, 'dev')] }, () => [], 'reviewed-by:dev');
      expect(result.opened).toHaveLength(1);
      expect(result.warnings[0]).toContain('reviewed-by:dev');
      expect(result.warnings[0]).toContain('incomplete');
    });

    it('does not display PRs that closed after being indexed by search', async () => {
      const result = await load({ 'review-requested:dev': [item(2)] }, (path) =>
        path.endsWith('/pulls/2')
          ? { state: 'closed', requested_reviewers: [{ login: 'dev' }] }
          : [],
      );
      expect(result.requested).toEqual([]);
    });

    it('paginates review threads before deciding whether a mention was answered', async () => {
      const pending = provider.getPullRequestInbox();
      http.expectOne('/api/forges/0/user').flush({ login: 'dev' });
      await tick();
      for (const request of http.match((request) => request.url.endsWith('/search/issues'))) {
        const items = request.request.params.get('q')!.includes('mentions:') ? [item(2)] : [];
        request.flush({ items, total_count: items.length, incomplete_results: false });
      }
      await tick();
      for (const request of http.match(() => true)) {
        if (request.request.url.endsWith('/pulls/2/comments'))
          request.flush(
            Array.from({ length: 100 }, (_, index) => ({
              id: index + 1,
              user: { login: 'colleague' },
              body: index ? 'Comment' : '@dev?',
              created_at: '2026-11-01T10:00:00Z',
            })),
          );
        else request.flush([]);
      }
      await tick();
      const second = http.expectOne((request) => request.url.endsWith('/pulls/2/comments'));
      expect(second.request.params.get('page')).toBe('2');
      second.flush([
        {
          id: 101,
          in_reply_to_id: 1,
          user: { login: 'dev' },
          body: 'Answered',
          created_at: '2026-11-02T10:00:00Z',
        },
      ]);
      expect((await pending).requested).toEqual([]);
    });
  });
});

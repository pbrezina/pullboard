import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { vi } from 'vitest';
import { ConfigService } from './config';
import { descriptionText, JiraService, selectTransition } from './jira';
import { AppConfig } from './models';
import { testConfig } from './testing';

const context = {
  boardId: 1,
  defaultSprintId: 2,
  accountId: 'account-123',
  username: 'dev@example.com',
  projectKey: 'DEV',
  sprints: [{ id: 2, name: 'Sprint 2', state: 'active' as const }],
};
const tick = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe('workflow transition selection', () => {
  it('matches destination status, not action name', () => {
    expect(
      selectTransition(
        [{ id: '77', name: 'Send for sign-off', to: { name: 'Review' } }],
        ['Review'],
      ).id,
    ).toBe('77');
  });
  it('respects ordered aliases and ignores casing', () => {
    expect(
      selectTransition(
        [
          { id: '1', name: 'Complete', to: { name: 'Closed' } },
          { id: '2', name: 'Finish', to: { name: 'DONE' } },
        ],
        ['Done', 'Closed'],
      ).id,
    ).toBe('2');
  });
  it('does not invent transitions or required field values', () => {
    expect(() => selectTransition([], ['Review'])).toThrow('No available transition');
    expect(() =>
      selectTransition(
        [
          {
            id: '1',
            name: 'Finish',
            to: { name: 'Done' },
            fields: { resolution: { required: true } },
          },
        ],
        ['Done'],
      ),
    ).toThrow('extra fields');
  });
});

describe('JiraService', () => {
  let http: HttpTestingController;
  let service: JiraService;
  let config: AppConfig;
  beforeEach(() => {
    config = structuredClone(testConfig);
    TestBed.configureTestingModule({
      providers: [
        provideHttpClient(),
        provideHttpClientTesting(),
        { provide: ConfigService, useValue: { load: async () => config } },
      ],
    });
    http = TestBed.inject(HttpTestingController);
    service = TestBed.inject(JiraService);
  });
  afterEach(() => http.verify());
  const path = (suffix: string) => `/api/jira${suffix}`;

  async function flushIdentity(): Promise<void> {
    await tick();
    const user = {
      accountId: context.accountId,
      emailAddress: config.jira.email,
      active: true,
    };
    http.expectOne(path('/rest/api/3/myself')).flush(user);
    http.expectOne((request) => request.url.endsWith('/user/search')).flush([user]);
    await tick();
  }

  it('discovers transition IDs from the issue workflow and uses Basic authentication', async () => {
    const pending = service.transition('DEV-1', 'review');
    await tick();
    const issue = http.expectOne((request) => request.url === path('/rest/api/3/issue/DEV-1'));
    expect(issue.request.headers.get('Authorization')).toBe(
      `Basic ${btoa('dev@example.com:test-only')}`,
    );
    issue.flush({ fields: { status: { name: 'To Do' } } });
    await tick();
    http
      .expectOne((request) => request.url.endsWith('/DEV-1/transitions'))
      .flush({ transitions: [{ id: '317', name: 'Request review', to: { name: 'Review' } }] });
    await tick();
    const transition = http.expectOne(path('/rest/api/3/issue/DEV-1/transitions'));
    expect(transition.request.method).toBe('POST');
    expect(transition.request.body).toEqual({ transition: { id: '317' } });
    transition.flush(null);
    await pending;
  });

  it('does not transition an already-correct issue', async () => {
    const pending = service.transition('DEV-1', 'inProgress');
    await tick();
    http
      .expectOne((request) => request.url.endsWith('/issue/DEV-1'))
      .flush({ fields: { status: { name: 'IN PROGRESS' } } });
    await pending;
    http.expectNone((request) => request.method === 'POST');
  });

  it('uses stable remote-link global IDs across retries', async () => {
    for (let index = 0; index < 2; index++) {
      const pending = service.addWebLink(
        'DEV-1',
        'https://github.com/org/repo/pull/42/?tab=files#diff',
      );
      await tick();
      const request = http.expectOne(path('/rest/api/3/issue/DEV-1/remotelink'));
      expect(request.request.body).toEqual({
        globalId: 'pullboard:https://github.com/org/repo/pull/42',
        object: { url: 'https://github.com/org/repo/pull/42', title: 'Upstream pull request' },
      });
      request.flush({ id: 10 });
      await pending;
    }
  });

  it('creates an assigned story with discovered points field and ADF description', async () => {
    const pending = service.createTicket({
      title: 'Track shutdown',
      description: 'Scope\nAcceptance criteria',
      storyPoints: 5,
    });
    await flushIdentity();
    http.expectNone((request) => request.url.includes('/sprint'));
    http
      .expectOne((request) => request.url.endsWith('/createmeta/DEV/issuetypes'))
      .flush({ issueTypes: [{ id: '10001', name: 'Story', subtask: false }], total: 1 });
    await tick();
    http
      .expectOne((request) => request.url.endsWith('/issuetypes/10001'))
      .flush({
        fields: [
          { fieldId: 'summary', name: 'Summary', required: true },
          { fieldId: 'assignee', name: 'Assignee', required: false },
          { fieldId: 'customfield_10016', name: 'Story Points', schema: { type: 'number' } },
        ],
        total: 3,
      });
    await tick();
    const request = http.expectOne(path('/rest/api/3/issue'));
    expect(request.request.method).toBe('POST');
    expect(request.request.body.fields.assignee).toEqual({ accountId: context.accountId });
    expect(request.request.body.fields.customfield_10016).toBe(5);
    expect(request.request.body.fields.issuetype).toEqual({ id: '10001' });
    expect(request.request.body.fields.description).toEqual({
      type: 'doc',
      version: 1,
      content: [
        { type: 'paragraph', content: [{ type: 'text', text: 'Scope' }] },
        { type: 'paragraph', content: [{ type: 'text', text: 'Acceptance criteria' }] },
      ],
    });
    request.flush({ key: 'DEV-12' });
    expect(await pending).toEqual({
      key: 'DEV-12',
      url: 'https://team.atlassian.net/browse/DEV-12',
    });
  });

  it('creates explicitly unassigned tickets without resolving an assignee or transitioning', async () => {
    const pending = service.createTicket(
      { title: 'Review PR org/repo#42: Shutdown', description: 'Review scope', storyPoints: 2 },
      true,
    );
    await tick();
    http
      .expectOne((request) => request.url.endsWith('/createmeta/DEV/issuetypes'))
      .flush({ issueTypes: [{ id: '10001', name: 'Story' }], total: 1 });
    await tick();
    http
      .expectOne((request) => request.url.endsWith('/issuetypes/10001'))
      .flush({
        fields: [
          { fieldId: 'summary', name: 'Summary', required: true },
          { fieldId: 'assignee', name: 'Assignee', required: false },
          { fieldId: 'customfield_10016', name: 'Story Points', schema: { type: 'number' } },
        ],
        total: 3,
      });
    await tick();
    const request = http.expectOne(path('/rest/api/3/issue'));
    expect(request.request.body.fields.assignee).toBeNull();
    expect(request.request.body.fields.status).toBeUndefined();
    request.flush({ key: 'DEV-12' });
    expect((await pending).key).toBe('DEV-12');
    http.expectNone(
      (request) =>
        request.url.includes('/myself') ||
        request.url.includes('/user/search') ||
        request.url.includes('/transitions'),
    );
  });

  it('queries active sprint tickets with enhanced token pagination and reads Web Links', async () => {
    vi.spyOn(service, 'getSprintContext').mockResolvedValue(context);
    const pending = service.getTickets();
    await tick();
    const first = http.expectOne((request) => request.url.endsWith('/search/jql'));
    expect(first.request.params.get('jql')).toContain('sprint = 2 AND assignee = "account-123"');
    expect(first.request.params.get('jql')).toContain('statusCategory != Done');
    expect(first.request.params.get('fields')).toBe('summary,status,description,issuetype');
    first.flush({
      issues: [
        {
          key: 'DEV-1',
          fields: {
            summary: 'One',
            description: {
              type: 'doc',
              content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Work scope' }] }],
            },
            status: { name: 'Review' },
            issuetype: { name: 'Epic' },
          },
        },
      ],
      nextPageToken: 'second',
    });
    await tick();
    const second = http.expectOne((request) => request.url.endsWith('/search/jql'));
    expect(second.request.params.get('nextPageToken')).toBe('second');
    second.flush({ issues: [], isLast: true });
    await tick();
    http.expectOne(path('/rest/api/3/issue/DEV-1/remotelink')).flush([
      { id: 1, object: { url: 'https://github.com/org/repo/pull/42', title: 'PR' } },
      { id: 2, object: { url: 'javascript:alert(1)' } },
    ]);
    const result = await pending;
    expect(result.tickets).toHaveLength(1);
    expect(result.tickets[0].description).toBe('Work scope');
    expect(result.tickets[0].issueType).toBe('Epic');
    expect(result.tickets[0].links).toEqual([
      { id: 1, url: 'https://github.com/org/repo/pull/42', title: 'PR' },
    ]);
  });

  it('resolves the configured email to the verified current account and active sprint', async () => {
    const pending = service.getSprintContext();
    await tick();
    http
      .expectOne(path('/rest/api/3/myself'))
      .flush({ accountId: context.accountId, emailAddress: testConfig.jira.email, active: true });
    http
      .expectOne((request) => request.url.endsWith('/user/search'))
      .flush([{ accountId: context.accountId, emailAddress: testConfig.jira.email, active: true }]);
    await tick();
    http
      .expectOne((request) => request.url.endsWith('/board/1/sprint'))
      .flush({ values: [{ id: 2, name: 'Sprint 2', state: 'active' }], isLast: true });
    expect(await pending).toEqual(context);
  });

  it('automatically selects the matching active sprint across all pages on a shared board', async () => {
    config.jira.sprintNamePattern = '%YEAR-IDM-SSSD-IDENTITY-S%NUMBER';
    const pending = service.getSprintContext();
    await flushIdentity();
    const first = http.expectOne((request) => request.url.endsWith('/board/1/sprint'));
    expect(first.request.params.get('state')).toBe('active,future');
    expect(first.request.params.get('startAt')).toBe('0');
    first.flush({
      values: [
        { id: 3, name: '2026-IDM-OTHER-S25', state: 'active' },
        { id: 4, name: '2026-IDM-SSSD-IDENTITY-S99', state: 'closed' },
      ],
      isLast: false,
    });
    await tick();
    const second = http.expectOne((request) => request.url.endsWith('/board/1/sprint'));
    expect(second.request.params.get('startAt')).toBe('2');
    second.flush({
      values: [{ id: 5, name: '2026-IDM-SSSD-IDENTITY-S23', state: 'active' }],
      isLast: true,
    });
    expect(await pending).toEqual({
      ...context,
      defaultSprintId: 5,
      sprints: [{ id: 5, name: '2026-IDM-SSSD-IDENTITY-S23', state: 'active' }],
    });
  });

  it.each([
    {
      scenario: 'only an unrelated sprint is active',
      sprints: [{ id: 3, name: '2026-IDM-OTHER-S23', state: 'active' }],
      message: 'No active sprint matches jira.sprintNamePattern',
    },
    {
      scenario: 'no sprint is active',
      sprints: [],
      message: 'No active sprint matches jira.sprintNamePattern',
    },
    {
      scenario: 'the matching sprint is closed',
      sprints: [{ id: 3, name: '2026-IDM-SSSD-IDENTITY-S23', state: 'closed' }],
      message: 'No active sprint matches jira.sprintNamePattern',
    },
    {
      scenario: 'several matching sprints are active',
      sprints: [
        { id: 3, name: '2026-IDM-SSSD-IDENTITY-S23', state: 'active' },
        { id: 4, name: '2026-IDM-SSSD-IDENTITY-S24', state: 'active' },
      ],
      message: 'Multiple active sprints match jira.sprintNamePattern',
    },
  ])('refuses to guess when $scenario', async ({ sprints, message }) => {
    config.jira.sprintNamePattern = '%YEAR-IDM-SSSD-IDENTITY-S%NUMBER';
    const pending = service.getSprintContext();
    await flushIdentity();
    http
      .expectOne((request) => request.url.endsWith('/board/1/sprint'))
      .flush({ values: sprints, isLast: true });
    expect((await pending).sprintWarning).toContain(message);
    expect((await pending).defaultSprintId).toBeUndefined();
  });

  it('retains the ambiguity error when no name pattern or ID is configured', async () => {
    const pending = service.getSprintContext();
    await flushIdentity();
    http
      .expectOne((request) => request.url.endsWith('/board/1/sprint'))
      .flush({
        values: [
          { id: 2, name: 'Sprint 2', state: 'active' },
          { id: 3, name: 'Sprint 3', state: 'active' },
        ],
        isLast: true,
      });
    expect((await pending).sprintWarning).toContain('configure jira.sprintNamePattern');
    expect((await pending).defaultSprintId).toBeUndefined();
  });

  it('lets an explicit active sprint ID override the name pattern', async () => {
    config.jira.sprintId = 2;
    config.jira.sprintNamePattern = '%YEAR-IDM-SSSD-IDENTITY-S%NUMBER';
    const pending = service.getSprintContext();
    await flushIdentity();
    http
      .expectOne((request) => request.url.endsWith('/board/1/sprint'))
      .flush({
        values: [
          { id: 2, name: 'Sprint 2', state: 'active' },
          { id: 3, name: '2026-IDM-SSSD-IDENTITY-S23', state: 'active' },
        ],
        isLast: true,
      });
    expect((await pending).defaultSprintId).toBe(2);
  });

  it('does not silently replace an invalid sprint ID with a name match', async () => {
    config.jira.sprintId = 999;
    config.jira.sprintNamePattern = '%YEAR-IDM-SSSD-IDENTITY-S%NUMBER';
    const pending = service.getSprintContext();
    await flushIdentity();
    http
      .expectOne((request) => request.url.endsWith('/board/1/sprint'))
      .flush({
        values: [{ id: 3, name: '2026-IDM-SSSD-IDENTITY-S23', state: 'active' }],
        isLast: true,
      });
    expect((await pending).sprintWarning).toContain('jira.sprintId must identify an active sprint');
    expect((await pending).defaultSprintId).toBeUndefined();
  });

  it('refuses creation in a sprint that is no longer active', async () => {
    vi.spyOn(service, 'getSprintContext').mockResolvedValue(context);
    const pending = service.assertSelectableSprint(2);
    const assertion = expect(pending).rejects.toThrow('no longer active or future');
    await tick();
    http.expectOne(path('/rest/agile/1.0/sprint/2')).flush({ id: 2, state: 'closed' });
    await assertion;
  });

  it('lists matching active and future sprints, excluding closed and unrelated ones', async () => {
    config.jira.sprintNamePattern = 'Team S%NUMBER';
    const pending = service.getSprintContext();
    await flushIdentity();
    http
      .expectOne((request) => request.url.endsWith('/board/1/sprint'))
      .flush({
        values: [
          { id: 3, name: 'Team S3', state: 'future' },
          { id: 2, name: 'Team S2', state: 'active' },
          { id: 1, name: 'Team S1', state: 'closed' },
          { id: 4, name: 'Other S4', state: 'active' },
        ],
        isLast: true,
      });
    expect((await pending).sprints.map((sprint) => sprint.id)).toEqual([2, 3]);
    expect((await pending).defaultSprintId).toBe(2);
  });

  it.each([null, 3])(
    'queries an explicit selection %s rather than the active sprint',
    async (selection) => {
      vi.spyOn(service, 'getSprintContext').mockResolvedValue({
        ...context,
        sprints: [...context.sprints, { id: 3, name: 'Next', state: 'future' }],
      });
      const pending = service.getTickets(selection);
      await tick();
      const request = http.expectOne((request) => request.url.endsWith('/search/jql'));
      expect(request.request.params.get('jql')).toContain(
        selection === null ? 'sprint IS EMPTY' : 'sprint = 3',
      );
      expect(request.request.params.get('jql')).toContain('project = DEV');
      expect(request.request.params.get('jql')).toContain('assignee = "account-123"');
      request.flush({ issues: [], isLast: true });
      expect((await pending).selection).toBe(selection);
    },
  );

  it('falls back to no sprint when no active sprint exists', async () => {
    vi.spyOn(service, 'getSprintContext').mockResolvedValue({
      ...context,
      defaultSprintId: undefined,
      sprints: [],
    });
    const pending = service.getTickets();
    await tick();
    const request = http.expectOne((request) => request.url.endsWith('/search/jql'));
    expect(request.request.params.get('jql')).toContain('sprint IS EMPTY');
    request.flush({ issues: [], isLast: true });
    expect((await pending).selection).toBeNull();
  });

  it('warns and falls back to unsprinted work if the viewed sprint disappears', async () => {
    vi.spyOn(service, 'getSprintContext').mockResolvedValue(context);
    const pending = service.getTickets(999);
    await tick();
    const request = http.expectOne((request) => request.url.endsWith('/search/jql'));
    expect(request.request.params.get('jql')).toContain('sprint IS EMPTY');
    request.flush({ issues: [], isLast: true });
    const result = await pending;
    expect(result.selection).toBeNull();
    expect(result.context.sprintWarning).toContain('no longer available');
  });

  it('adds tickets to the chosen future sprint, not the active sprint', async () => {
    vi.spyOn(service, 'getSprintContext').mockResolvedValue({
      ...context,
      sprints: [...context.sprints, { id: 3, name: 'Next', state: 'future' }],
    });
    const pending = service.addToSprint('DEV-1', 3);
    await tick();
    http.expectOne(path('/rest/agile/1.0/sprint/3')).flush({ id: 3, state: 'future' });
    await tick();
    const request = http.expectOne(path('/rest/agile/1.0/sprint/3/issue'));
    expect(request.request.body).toEqual({ issues: ['DEV-1'] });
    request.flush(null);
    await pending;
  });

  it('rejects sprint IDs outside the configured board before any request', async () => {
    vi.spyOn(service, 'getSprintContext').mockResolvedValue(context);
    await expect(service.assertSelectableSprint(999)).rejects.toThrow('configured board');
    http.expectNone(() => true);
  });
});

describe('JIRA description snippets', () => {
  it('handles absent and legacy descriptions', () => {
    expect(descriptionText(null)).toBe('');
    expect(descriptionText('Plain text')).toBe('Plain text');
  });
  it('preserves paragraph boundaries and never interprets HTML', () => {
    expect(
      descriptionText({
        type: 'doc',
        content: [
          { type: 'paragraph', content: [{ type: 'text', text: '<script>text</script>' }] },
          {
            type: 'paragraph',
            content: [
              { type: 'mention', attrs: { text: '@Dev' } },
              { type: 'hardBreak' },
              { type: 'text', text: 'Next line' },
            ],
          },
        ],
      }),
    ).toBe('<script>text</script>\n@Dev\nNext line\n');
  });
});

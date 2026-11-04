import { HttpClient, provideHttpClient, withInterceptors } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { firstValueFrom } from 'rxjs';
import { vi } from 'vitest';
import { ConfigService } from './config';
import { ForgeAggregatorService } from './forge-aggregator';
import { JiraService } from './jira';
import { readOnlyForges } from './read-only-forges';
import { SyncService } from './sync';
import { TEST_MODE, testModeWrites } from './test-mode';
import { testConfig, testPr } from './testing';

describe('test mode HTTP and workflow safety', () => {
  let http: HttpTestingController;
  let jira: JiraService;
  let sync: SyncService;
  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [
        provideHttpClient(withInterceptors([readOnlyForges, testModeWrites])),
        provideHttpClientTesting(),
        { provide: TEST_MODE, useValue: true },
        { provide: ConfigService, useValue: { load: async () => testConfig } },
        {
          provide: ForgeAggregatorService,
          useValue: {
            getPullRequestStatus: async () => ({ state: 'open', draft: false }),
          },
        },
      ],
    });
    http = TestBed.inject(HttpTestingController);
    jira = TestBed.inject(JiraService);
    sync = TestBed.inject(SyncService);
  });
  afterEach(() => http.verify());

  it.each(['POST', 'PUT', 'PATCH', 'DELETE'])(
    'blocks accidental JIRA %s at the HTTP boundary',
    async (method) => {
      await expect(
        firstValueFrom(
          TestBed.inject(HttpClient).request(method, '/api/jira/rest/api/3/issue', { body: {} }),
        ),
      ).rejects.toThrow('Test mode blocks');
      http.expectNone(() => true);
    },
  );

  it('still blocks forge mutations', async () => {
    await expect(
      firstValueFrom(TestBed.inject(HttpClient).post('/api/forges/0/repos/org/repo/pulls/42', {})),
    ).rejects.toThrow('read-only');
    http.expectNone(() => true);
  });

  it('allows real JIRA reads and live AI drafting requests', async () => {
    const links = jira.getWebLinks('DEV-1');
    await vi.waitFor(() => {
      const request = http.expectOne('/api/jira/rest/api/3/issue/DEV-1/remotelink');
      expect(request.request.method).toBe('GET');
      request.flush([{ id: 1, object: { url: testPr.url, title: 'PR' } }]);
    });
    expect(await links).toHaveLength(1);
    const draft = firstValueFrom(
      TestBed.inject(HttpClient).post('/api/openai/v1/chat/completions', {}),
    );
    http.expectOne('/api/openai/v1/chat/completions').flush({ choices: [] });
    await draft;
  });

  it('simulates creation and returns a clearly fake, non-navigable receipt without any writes', async () => {
    const created = vi.fn();
    const receipt = await sync.create(
      testPr,
      { title: 'Track work', description: 'Scope', storyPoints: 3 },
      created,
    );
    expect(receipt).toEqual({ key: 'SIMULATED-1', url: '', simulated: true });
    expect(created).toHaveBeenCalledWith(receipt);
    http.expectNone(() => true);
  });

  it('simulates linking an existing ticket and resuming without a mutation', async () => {
    expect(
      await sync.resume(
        testPr,
        { key: 'DEV-1', url: 'https://team.atlassian.net/browse/DEV-1' },
        2,
      ),
    ).toMatchObject({ key: 'DEV-1', simulated: true });
    http.expectNone(() => true);
  });

  it('simulates direct linking and every transition, including closing', async () => {
    await jira.addWebLink('DEV-1', testPr.url);
    for (const phase of ['inProgress', 'review', 'done'] as const)
      await jira.transition('DEV-1', phase);
    http.expectNone(() => true);
  });

  it('reads sprint state but never writes sprint membership', async () => {
    vi.spyOn(jira, 'getSprintContext').mockResolvedValue({
      boardId: 1,
      accountId: 'account-1',
      username: 'Dev',
      projectKey: 'DEV',
      sprints: [{ id: 2, name: 'Current', state: 'active' }],
      defaultSprintId: 2,
    });
    const pending = jira.addToSprint('DEV-1', 2);
    await vi.waitFor(() => {
      http.expectOne('/api/jira/rest/agile/1.0/sprint/2').flush({ id: 2, state: 'active' });
    });
    await pending;
    http.expectNone((request) => request.method !== 'GET');
  });
});

describe('normal mode HTTP boundary', () => {
  it('allows normal JIRA writes when test mode is not enabled', async () => {
    TestBed.configureTestingModule({
      providers: [
        provideHttpClient(withInterceptors([testModeWrites])),
        provideHttpClientTesting(),
        { provide: TEST_MODE, useValue: false },
      ],
    });
    const http = TestBed.inject(HttpTestingController);
    const pending = firstValueFrom(
      TestBed.inject(HttpClient).post('/api/jira/rest/api/3/issue', {}),
    );
    http.expectOne('/api/jira/rest/api/3/issue').flush({ key: 'DEV-1' });
    expect(await pending).toEqual({ key: 'DEV-1' });
    http.verify();
  });
});

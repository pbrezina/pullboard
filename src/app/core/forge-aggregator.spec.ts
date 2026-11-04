import { HttpErrorResponse } from '@angular/common/http';
import { TestBed } from '@angular/core/testing';
import { vi } from 'vitest';
import { ConfigService } from './config';
import { FORGE_PROVIDER_FACTORIES, ForgeAggregatorService } from './forge-aggregator';
import { IForgeProvider } from './models';
import { testConfig, testPr } from './testing';

describe('ForgeAggregatorService', () => {
  let service: ForgeAggregatorService;
  const first = {
    id: 'forge-0',
    label: 'First forge',
    getUsername: vi.fn(),
    supportsUrl: vi.fn(),
    getOpenPullRequests: vi.fn(),
    getPullRequestInbox: vi.fn(),
    getPullRequestStatus: vi.fn(),
  } satisfies IForgeProvider;
  const second = {
    id: 'forge-1',
    label: 'Second forge',
    getUsername: vi.fn(),
    supportsUrl: vi.fn(),
    getOpenPullRequests: vi.fn(),
    getPullRequestInbox: vi.fn(),
    getPullRequestStatus: vi.fn(),
  } satisfies IForgeProvider;
  beforeEach(() => {
    vi.resetAllMocks();
    first.getUsername.mockResolvedValue('first-user');
    second.getUsername.mockResolvedValue('second-user');
    first.supportsUrl.mockReturnValue(true);
    second.supportsUrl.mockReturnValue(true);
    first.getOpenPullRequests.mockResolvedValue([testPr]);
    second.getOpenPullRequests.mockResolvedValue([]);
    first.getPullRequestInbox.mockResolvedValue({
      opened: [{ ...testPr, needsAction: false }],
      requested: [],
      warnings: ['Partial timeline'],
    });
    second.getPullRequestInbox.mockResolvedValue({ opened: [], requested: [], warnings: [] });
    first.getPullRequestStatus.mockResolvedValue({ state: 'open', draft: false });
    second.getPullRequestStatus.mockResolvedValue({ state: 'merged', draft: false });
    TestBed.configureTestingModule({
      providers: [
        {
          provide: ConfigService,
          useValue: {
            load: async () => ({
              ...testConfig,
              forges: [testConfig.forges[0], testConfig.forges[0]],
            }),
          },
        },
        {
          provide: FORGE_PROVIDER_FACTORIES,
          useValue: [
            {
              type: 'github',
              create: (_config: unknown, index: number) => (index === 0 ? first : second),
            },
          ],
        },
      ],
    });
    service = TestBed.inject(ForgeAggregatorService);
  });
  it('aggregates independent strategies and preserves successful data when one fails', async () => {
    second.getOpenPullRequests.mockRejectedValue(new Error('Unavailable account'));
    const result = await service.getOpenPullRequests();
    expect(result.pullRequests).toEqual([testPr]);
    expect(result.warnings[0]).toContain('Unavailable account');
    expect(result.identities.map((identity) => identity.username)).toEqual([
      'first-user',
      'second-user',
    ]);
  });
  it('tries another matching account for inaccessible Web Links', async () => {
    first.getPullRequestStatus.mockRejectedValue(new HttpErrorResponse({ status: 404 }));
    expect(await service.getPullRequestStatus(testPr.url)).toEqual({
      state: 'merged',
      draft: false,
    });
    expect(second.getPullRequestStatus).toHaveBeenCalledWith(testPr.url);
  });
  it('aggregates inboxes and keeps successful accounts visible when another account fails', async () => {
    second.getPullRequestInbox.mockRejectedValue(new Error('Unavailable account'));
    const result = await service.getPullRequestInbox();
    expect(result.opened).toHaveLength(1);
    expect(result.warnings.join(' ')).toContain('Partial timeline');
    expect(result.warnings.join(' ')).toContain('Unavailable account');
    expect(result.identities.map((identity) => identity.username)).toEqual([
      'first-user',
      'second-user',
    ]);
  });
  it('pins direct synchronization checks to the PR account', async () => {
    expect(await service.getPullRequestStatus(testPr.url, 'forge-0')).toEqual({
      state: 'open',
      draft: false,
    });
    expect(second.getPullRequestStatus).not.toHaveBeenCalled();
  });
  it('does not retry rate limits across accounts', async () => {
    first.getPullRequestStatus.mockRejectedValue(new HttpErrorResponse({ status: 429 }));
    await expect(service.getPullRequestStatus(testPr.url)).rejects.toMatchObject({ status: 429 });
    expect(second.getPullRequestStatus).not.toHaveBeenCalled();
  });
  it('rejects unrecognized links without any provider HTTP call', async () => {
    first.supportsUrl.mockReturnValue(false);
    second.supportsUrl.mockReturnValue(false);
    await expect(service.getPullRequestStatus('https://unconfigured.example/pr/1')).rejects.toThrow(
      'No configured forge',
    );
    expect(first.getPullRequestStatus).not.toHaveBeenCalled();
  });
});

import { TestBed } from '@angular/core/testing';
import { vi } from 'vitest';
import { ForgeAggregatorService } from './forge-aggregator';
import { JiraService } from './jira';
import { PartialSyncError, SyncService } from './sync';
import { testPr } from './testing';

describe('JIRA synchronization', () => {
  const receipt = { key: 'DEV-9', url: 'https://team.atlassian.net/browse/DEV-9' };
  const suggestion = { title: 'Track work', description: 'Work scope', storyPoints: 3 };
  const jira = {
    createTicket: vi.fn(),
    addWebLink: vi.fn(),
    addToSprint: vi.fn(),
    assertSelectableSprint: vi.fn(),
    transition: vi.fn(),
  };
  const forges = { getPullRequestStatus: vi.fn() };
  let service: SyncService;
  beforeEach(() => {
    vi.resetAllMocks();
    jira.createTicket.mockResolvedValue(receipt);
    jira.addWebLink.mockResolvedValue(undefined);
    jira.addToSprint.mockResolvedValue(undefined);
    jira.assertSelectableSprint.mockResolvedValue(undefined);
    jira.transition.mockResolvedValue(undefined);
    forges.getPullRequestStatus.mockResolvedValue({ state: 'open', draft: false });
    TestBed.configureTestingModule({
      providers: [
        { provide: JiraService, useValue: jira },
        { provide: ForgeAggregatorService, useValue: forges },
      ],
    });
    service = TestBed.inject(SyncService);
  });
  it('creates once, saves the receipt, links, sprints and transitions ready PRs', async () => {
    const save = vi.fn();
    expect(await service.create(testPr, suggestion, save, 2)).toEqual(receipt);
    expect(save).toHaveBeenCalledWith(receipt);
    expect(jira.createTicket).toHaveBeenCalledExactlyOnceWith(suggestion);
    expect(jira.addWebLink).toHaveBeenCalledWith('DEV-9', testPr.url);
    expect(jira.assertSelectableSprint).toHaveBeenCalledWith(2);
    expect(jira.addToSprint).toHaveBeenCalledWith('DEV-9', 2);
    expect(jira.transition).toHaveBeenCalledWith('DEV-9', 'review');
  });
  it('uses fresh draft status rather than stale UI data', async () => {
    forges.getPullRequestStatus.mockResolvedValue({ state: 'open', draft: true });
    await service.resume(testPr, receipt);
    expect(jira.transition).toHaveBeenCalledWith('DEV-9', 'inProgress');
    expect(jira.addToSprint).not.toHaveBeenCalled();
    expect(jira.createTicket).not.toHaveBeenCalled();
  });
  it('preserves the created key when a later step fails and resumes without another creation', async () => {
    jira.transition.mockRejectedValueOnce(new Error('workflow denied'));
    const save = vi.fn();
    let partial: unknown;
    try {
      await service.create(testPr, suggestion, save);
    } catch (error) {
      partial = error;
    }
    expect(partial).toBeInstanceOf(PartialSyncError);
    expect((partial as PartialSyncError).receipt).toEqual(receipt);
    expect(save).toHaveBeenCalledWith(receipt);
    await service.resume(testPr, receipt, 2);
    expect(jira.createTicket).toHaveBeenCalledTimes(1);
  });
  it('refuses a closed PR before creating anything', async () => {
    forges.getPullRequestStatus.mockResolvedValue({ state: 'merged', draft: false });
    await expect(service.create(testPr, suggestion, vi.fn())).rejects.toThrow('no longer open');
    expect(jira.createTicket).not.toHaveBeenCalled();
    expect(jira.addWebLink).not.toHaveBeenCalled();
  });
  it('creates without a sprint by default and never checks or assigns a sprint', async () => {
    await service.create(testPr, suggestion, vi.fn());
    expect(jira.createTicket).toHaveBeenCalledOnce();
    expect(jira.assertSelectableSprint).not.toHaveBeenCalled();
    expect(jira.addToSprint).not.toHaveBeenCalled();
  });
  it('rejects an unavailable selected sprint before creating a ticket', async () => {
    jira.assertSelectableSprint.mockRejectedValue(new Error('Sprint closed'));
    await expect(service.create(testPr, suggestion, vi.fn(), 3)).rejects.toThrow('Sprint closed');
    expect(jira.createTicket).not.toHaveBeenCalled();
  });
  it('never resumes a simulated receipt in normal mode', async () => {
    await expect(
      service.resume(testPr, { key: 'SIMULATED-1', url: '', simulated: true }),
    ).rejects.toThrow('synchronization is incomplete');
    expect(jira.addWebLink).not.toHaveBeenCalled();
    expect(jira.addToSprint).not.toHaveBeenCalled();
    expect(jira.transition).not.toHaveBeenCalled();
  });
});

import { HttpErrorResponse } from '@angular/common/http';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { of, Subject } from 'rxjs';
import { vi } from 'vitest';
import { ConfigService } from './config';
import { JiraService } from './jira';
import { LlmService } from './llm';
import { PrJiraLinks } from './pr-jira-links';
import { ReviewTickets } from './review-tickets';
import { TEST_MODE } from './test-mode';
import { testConfig, testPr } from './testing';
import { TicketCreation } from './models';

const storageKey = 'pullboard:reviews:v1:https://team.atlassian.net:DEV:dev@example.com';
const receipt = { key: 'DEV-12', url: 'https://team.atlassian.net/browse/DEV-12' };
const suggestion = {
  title: 'Check shutdown',
  description: 'Inspect worker shutdown and verify cleanup.',
  storyPoints: 2,
};

describe('Review ticket creation', () => {
  const jira = {
    getSprintContext: vi.fn(),
    createTicket: vi.fn(),
    addWebLink: vi.fn(),
    assertSelectableSprint: vi.fn(),
    addToSprint: vi.fn(),
    transition: vi.fn(),
  };
  const llm = { suggest: vi.fn() };
  const dialogs = { open: vi.fn() };
  let reviews: ReviewTickets;

  beforeEach(() => {
    localStorage.clear();
    vi.resetAllMocks();
    jira.getSprintContext.mockResolvedValue({
      sprints: [{ id: 2, name: 'Sprint', state: 'active' }],
      defaultSprintId: 2,
    });
    jira.createTicket.mockResolvedValue(receipt);
    jira.addWebLink.mockResolvedValue(undefined);
    jira.addToSprint.mockResolvedValue(undefined);
    jira.assertSelectableSprint.mockResolvedValue(undefined);
    llm.suggest.mockResolvedValue(suggestion);
    dialogs.open.mockReturnValue({
      afterClosed: () => of({ ...suggestion, title: 'Edited review scope' }),
    });
    TestBed.configureTestingModule({
      providers: [
        { provide: ConfigService, useValue: { load: async () => testConfig } },
        { provide: JiraService, useValue: jira },
        { provide: LlmService, useValue: llm },
        { provide: MatDialog, useValue: dialogs },
      ],
    });
  });
  afterEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });
  const service = () => (reviews = TestBed.inject(ReviewTickets));

  it('does no AI or remote work while restoring local receipts', async () => {
    await service().initialize();
    expect(llm.suggest).not.toHaveBeenCalled();
    expect(jira.getSprintContext).not.toHaveBeenCalled();
    expect(jira.createTicket).not.toHaveBeenCalled();
  });

  it('confirms editable AI suggestions and creates an unassigned review ticket with a fixed prefix and Web Link', async () => {
    await service().create(testPr);
    expect(llm.suggest).toHaveBeenCalledWith(testPr, true);
    expect(dialogs.open).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({
        closeOnNavigation: false,
        data: expect.objectContaining({
          titlePrefix: 'Review PR org/repo#42',
          suggestion,
          defaultSprintId: 2,
        }),
      }),
    );
    expect(jira.createTicket).toHaveBeenCalledWith(
      { ...suggestion, title: 'Review PR org/repo#42: Edited review scope' },
      true,
    );
    expect(jira.addWebLink).toHaveBeenCalledWith(receipt.key, testPr.url);
    expect(jira.transition).not.toHaveBeenCalled();
    expect(jira.addToSprint).not.toHaveBeenCalled();
    expect(reviews.receipts()[testPr.url]).toEqual([receipt]);
    expect(TestBed.inject(PrJiraLinks).links()[testPr.url]).toEqual([receipt]);
    expect(localStorage.getItem(storageKey)).not.toContain('test-only');
  });

  it('never creates a ticket before confirmation and does nothing when cancelled', async () => {
    const confirmation = new Subject<TicketCreation | undefined>();
    dialogs.open.mockReturnValue({ afterClosed: () => confirmation });
    const pending = service().create(testPr);
    await vi.waitFor(() => expect(dialogs.open).toHaveBeenCalled());
    expect(jira.createTicket).not.toHaveBeenCalled();
    expect(reviews.busy()).toBe(true);
    await reviews.create(testPr);
    expect(llm.suggest).toHaveBeenCalledTimes(1);
    confirmation.next(undefined);
    confirmation.complete();
    await pending;
    expect(jira.createTicket).not.toHaveBeenCalled();
    expect(reviews.busy()).toBe(false);
  });

  it('allows additional tickets with the earlier tickets disclosed in confirmation', async () => {
    await service().create(testPr);
    const second = { key: 'DEV-13', url: 'https://team.atlassian.net/browse/DEV-13' };
    jira.createTicket.mockResolvedValueOnce(second);
    await reviews.create(testPr);
    expect(dialogs.open).toHaveBeenLastCalledWith(
      expect.anything(),
      expect.objectContaining({
        data: expect.objectContaining({ previousTickets: [receipt] }),
      }),
    );
    expect(reviews.receipts()[testPr.url]).toEqual([receipt, second]);
    expect(jira.createTicket).toHaveBeenCalledTimes(2);
  });

  it('persists the key before linking and resumes partial work without creating another ticket', async () => {
    jira.addWebLink.mockImplementationOnce(async () => {
      expect(JSON.parse(localStorage.getItem(storageKey)!).pending[testPr.url]).toMatchObject({
        status: 'partial',
        receipt,
      });
      throw new Error('Link denied');
    });
    await service().create(testPr);
    expect(reviews.states()[testPr.url]).toMatchObject({
      status: 'partial',
      receipt,
      error: 'Link denied',
    });
    await reviews.create(testPr);
    expect(jira.createTicket).toHaveBeenCalledTimes(1);
    await reviews.resume(testPr);
    expect(jira.createTicket).toHaveBeenCalledTimes(1);
    expect(jira.addWebLink).toHaveBeenCalledTimes(2);
    expect(reviews.states()[testPr.url].status).toBe('complete');
  });

  it('checks and preserves the exact optional sprint without changing ticket status', async () => {
    dialogs.open.mockReturnValue({ afterClosed: () => of({ ...suggestion, sprintId: 2 }) });
    jira.addToSprint.mockRejectedValueOnce(new Error('Sprint unavailable'));
    await service().create(testPr);
    expect(jira.assertSelectableSprint).toHaveBeenCalledWith(2);
    expect(jira.addToSprint).toHaveBeenCalledWith(receipt.key, 2);
    expect(reviews.states()[testPr.url]).toMatchObject({ status: 'partial', sprintId: 2 });
    await reviews.resume(testPr);
    expect(jira.addToSprint).toHaveBeenLastCalledWith(receipt.key, 2);
    expect(jira.createTicket).toHaveBeenCalledTimes(1);
    expect(jira.transition).not.toHaveBeenCalled();
  });

  it('does not create anything when a selected sprint is invalid', async () => {
    dialogs.open.mockReturnValue({ afterClosed: () => of({ ...suggestion, sprintId: 999 }) });
    jira.assertSelectableSprint.mockRejectedValueOnce(new Error('Invalid sprint'));
    await service().create(testPr);
    expect(jira.createTicket).not.toHaveBeenCalled();
    expect(reviews.states()[testPr.url]).toMatchObject({
      status: 'error',
      error: 'Invalid sprint',
    });
  });

  it('restores validated receipts and partial operations without scanning JIRA', async () => {
    localStorage.setItem(
      storageKey,
      JSON.stringify({
        receipts: {
          [testPr.url]: [
            receipt,
            { key: 'DEV-99', url: 'https://evil.example/browse/DEV-99' },
            null,
          ],
        },
        pending: { [testPr.url]: { status: 'partial', receipt, sprintId: 2 } },
      }),
    );
    await service().initialize();
    expect(reviews.receipts()[testPr.url]).toEqual([receipt]);
    expect(TestBed.inject(PrJiraLinks).links()[testPr.url]).toEqual([receipt]);
    expect(jira.getSprintContext).not.toHaveBeenCalled();
    await reviews.resume(testPr);
    expect(jira.addToSprint).toHaveBeenCalledWith(receipt.key, 2);
    expect(jira.createTicket).not.toHaveBeenCalled();
  });

  it.each([0, 503])(
    'blocks unsafe retries after an unknown creation outcome (%s)',
    async (status) => {
      jira.createTicket.mockImplementationOnce(async () => {
        expect(JSON.parse(localStorage.getItem(storageKey)!).pending[testPr.url].status).toBe(
          'uncertain',
        );
        throw new HttpErrorResponse({ status });
      });
      await service().create(testPr);
      expect(reviews.states()[testPr.url].status).toBe('uncertain');
      await reviews.create(testPr);
      expect(jira.createTicket).toHaveBeenCalledTimes(1);
      const confirm = vi
        .spyOn(window, 'confirm')
        .mockReturnValueOnce(false)
        .mockReturnValueOnce(true);
      reviews.allowRetry(testPr);
      expect(reviews.states()[testPr.url].status).toBe('uncertain');
      reviews.allowRetry(testPr);
      expect(confirm).toHaveBeenCalledTimes(2);
      expect(reviews.states()[testPr.url]).toBeUndefined();
    },
  );

  it('supports manual entry after an AI failure', async () => {
    llm.suggest.mockRejectedValueOnce(new Error('AI unavailable'));
    await service().create(testPr);
    expect(reviews.states()[testPr.url].status).toBe('error');
    expect(jira.createTicket).not.toHaveBeenCalled();
    await reviews.create(testPr, true);
    expect(llm.suggest).toHaveBeenCalledTimes(1);
    expect(dialogs.open).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({
        data: expect.objectContaining({ generated: false }),
      }),
    );
    expect(jira.createTicket).toHaveBeenCalledTimes(1);
  });

  it('keeps test-mode simulations separate from real receipts and mutations', async () => {
    TestBed.overrideProvider(TEST_MODE, { useValue: true });
    localStorage.setItem(storageKey, '{"keep":"real receipts"}');
    jira.createTicket.mockResolvedValueOnce({ key: 'SIMULATED-1', url: '', simulated: true });
    await service().create(testPr);
    expect(reviews.states()[testPr.url].receipt?.simulated).toBe(true);
    expect(reviews.receipts()).toEqual({});
    expect(TestBed.inject(PrJiraLinks).links()).toEqual({});
    expect(jira.addWebLink).not.toHaveBeenCalled();
    expect(jira.addToSprint).not.toHaveBeenCalled();
    expect(jira.transition).not.toHaveBeenCalled();
    expect(localStorage.getItem(storageKey)).toBe('{"keep":"real receipts"}');
  });
});

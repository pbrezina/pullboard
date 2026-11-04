import { inject, Service } from '@angular/core';
import { ForgeAggregatorService } from './forge-aggregator';
import { JiraService } from './jira';
import { SyncReceipt, TicketSuggestion, UnifiedPullRequest } from './models';
import { TEST_MODE } from './test-mode';

export class PartialSyncError extends Error {
  constructor(
    readonly receipt: SyncReceipt,
    readonly causeError: unknown,
  ) {
    super(
      `Ticket ${receipt.key} exists, but synchronization is incomplete. Resume this ticket; do not create another.`,
    );
  }
}

@Service()
export class SyncService {
  private readonly jira = inject(JiraService);
  private readonly forges = inject(ForgeAggregatorService);
  private readonly testMode = inject(TEST_MODE);

  private async phase(pr: UnifiedPullRequest): Promise<'inProgress' | 'review'> {
    const status = await this.forges.getPullRequestStatus(pr.url, pr.providerId);
    if (status.state !== 'open')
      throw new Error('This PR is no longer open. Refresh before syncing.');
    return status.draft ? 'inProgress' : 'review';
  }

  async create(
    pr: UnifiedPullRequest,
    suggestion: TicketSuggestion,
    onCreated: (receipt: SyncReceipt) => void,
    sprintId?: number,
  ): Promise<SyncReceipt> {
    await this.phase(pr);
    if (sprintId !== undefined) await this.jira.assertSelectableSprint(sprintId);
    const receipt = await this.jira.createTicket(suggestion);
    // Preserve the key immediately: later JIRA calls are separate, non-transactional requests.
    onCreated(receipt);
    if (this.testMode) return { ...receipt, simulated: true };
    return this.resume(pr, receipt, sprintId);
  }

  async resume(
    pr: UnifiedPullRequest,
    receipt: SyncReceipt,
    sprintId?: number,
  ): Promise<SyncReceipt> {
    try {
      const phase = await this.phase(pr);
      if (this.testMode) return { ...receipt, simulated: true };
      if (receipt.simulated) throw new Error('A simulated receipt cannot be used in normal mode.');
      // Link first so a later failure never leaves an invisible association.
      await this.jira.addWebLink(receipt.key, pr.url);
      if (sprintId !== undefined) await this.jira.addToSprint(receipt.key, sprintId);
      await this.jira.transition(receipt.key, phase);
      return receipt;
    } catch (error) {
      throw new PartialSyncError(receipt, error);
    }
  }
}

import { Service, signal } from '@angular/core';
import { SyncReceipt } from './models';

/** Shared, locally known associations only. Never scans or queries JIRA. */
@Service()
export class PrJiraLinks {
  readonly links = signal<Record<string, SyncReceipt[]>>({});

  remember(url: string, receipts: readonly SyncReceipt[]): void {
    const real = receipts.filter((receipt) => !receipt.simulated);
    if (!real.length) return;
    this.links.update((links) => ({
      ...links,
      [url]: [
        ...new Map([...(links[url] ?? []), ...real].map((entry) => [entry.key, entry])).values(),
      ],
    }));
  }
}

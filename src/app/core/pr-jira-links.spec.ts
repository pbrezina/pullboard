import { TestBed } from '@angular/core/testing';
import { PrJiraLinks } from './pr-jira-links';
import { testPr } from './testing';

describe('Locally known PR JIRA links', () => {
  it('shares and deduplicates receipts without depending on any remote service', () => {
    const links = TestBed.inject(PrJiraLinks);
    const receipt = { key: 'DEV-1', url: 'https://team.atlassian.net/browse/DEV-1' };
    links.remember(testPr.url, [receipt, receipt]);
    links.remember(testPr.url, [{ key: 'DEV-2', url: 'https://team.atlassian.net/browse/DEV-2' }]);
    expect(links.links()[testPr.url]).toEqual([
      receipt,
      { key: 'DEV-2', url: 'https://team.atlassian.net/browse/DEV-2' },
    ]);
  });

  it('never records simulated associations', () => {
    const links = TestBed.inject(PrJiraLinks);
    links.remember(testPr.url, [{ key: 'SIMULATED-1', url: '', simulated: true }]);
    expect(links.links()).toEqual({});
  });
});

import {
  oldestFirst,
  relativeAge,
  waitingFirst,
  waitingSince,
  actionFirst,
  isReviewExcluded,
  isAgedReview,
} from './pull-request-order';
import { testPr } from './testing';
import { PullRequestWorkItem } from './models';

describe('PR ordering and activity ages', () => {
  const a: PullRequestWorkItem = {
    ...testPr,
    needsAction: false,
    reviewStatus: 'outstanding',
    reviewRequestedAt: '2026-11-02T10:00:00Z',
    updatedAt: '2026-11-04T10:00:00Z',
  };
  const b: PullRequestWorkItem = {
    ...a,
    id: 'b',
    reviewRequestedAt: '2026-11-03T10:00:00Z',
    updatedAt: '2026-11-03T10:00:00Z',
  };
  it('prioritizes waiting time rather than unrelated recent activity', () => {
    expect([b, a].sort(waitingFirst)).toEqual([a, b]);
    expect([a, b].sort(oldestFirst)).toEqual([b, a]);
  });
  it('uses the oldest pending mention/request and last activity for given reviews', () => {
    expect(waitingSince({ ...a, mentionedAt: '2026-11-01T10:00:00Z' })).toBe(
      '2026-11-01T10:00:00Z',
    );
    const given = {
      ...b,
      reviewStatus: 'given' as const,
      reviewRequestedAt: '2020-01-01T00:00:00Z',
    };
    expect(waitingSince(given)).toBeUndefined();
    expect([a, given].sort(waitingFirst)).toEqual([a, given]);
  });
  it('falls back to last activity when a request timestamp is unknown', () => {
    expect([a, { ...b, reviewRequestedAt: undefined }].sort(waitingFirst)[0]).toBe(a);
  });
  it('formats ages without starting any timer', () => {
    const now = Date.parse('2026-11-04T10:00:00Z');
    expect(relativeAge('2026-11-04T09:59:50Z', now)).toBe('just now');
    expect(relativeAge('2026-11-04T09:57:00Z', now)).toBe('3 minutes');
    expect(relativeAge('2026-11-04T08:00:00Z', now)).toBe('2 hours');
    expect(relativeAge('2026-11-01T10:00:00Z', now)).toBe('3 days');
    expect(relativeAge('2026-11-03T10:00:00Z', now)).toBe('1 day');
  });

  it('puts pending reviews or mentions before completed reviews regardless of age', () => {
    const oldGiven = {
      ...a,
      id: 'given',
      reviewStatus: 'given' as const,
      updatedAt: '2020-01-01T00:00:00Z',
    };
    const mention = { ...b, reviewStatus: undefined, mentionedAt: b.reviewRequestedAt };
    expect([oldGiven, mention, a].sort(waitingFirst)).toEqual([a, mention, oldGiven]);
  });
  it('sorts opened PRs by action, then oldest activity with deterministic ties', () => {
    const action = { ...a, needsAction: true };
    const olderAction = { ...b, needsAction: true };
    const oldest = { ...a, id: 'oldest', updatedAt: '2020-01-01T00:00:00Z' };
    expect([oldest, action, olderAction].sort(actionFirst)).toEqual([olderAction, action, oldest]);
  });
  it('excludes only exact Blocked/Deferred label names ignoring case', () => {
    for (const label of ['Blocked', 'DEFERRED', 'blocked'])
      expect(isReviewExcluded({ ...a, labels: [label] })).toBe(true);
    for (const label of ['Not Blocked', 'Deferred-work', 'Accepted'])
      expect(isReviewExcluded({ ...a, labels: [label] })).toBe(false);
  });
  it('uses a strict 30-day threshold and the relevant waiting/activity timestamp', () => {
    const now = Date.parse('2026-11-04T10:00:00Z');
    expect(isAgedReview({ ...a, reviewRequestedAt: '2026-10-05T10:00:00Z' }, now)).toBe(false);
    expect(isAgedReview({ ...a, reviewRequestedAt: '2026-10-05T09:59:59Z' }, now)).toBe(true);
    expect(isAgedReview({ ...a, updatedAt: '2020-01-01T00:00:00Z' }, now)).toBe(false);
    expect(
      isAgedReview({ ...a, reviewStatus: 'given', updatedAt: '2020-01-01T00:00:00Z' }, now),
    ).toBe(true);
    expect(isAgedReview({ ...a, reviewRequestedAt: 'invalid' }, now)).toBe(false);
  });
});

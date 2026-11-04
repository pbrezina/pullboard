import { PullRequestWorkItem, UnifiedPullRequest } from './models';

const labelOrder = new Intl.Collator('en', { sensitivity: 'base' });
export const sortedLabels = (labels: string[]): string[] => [...labels].sort(labelOrder.compare);
export const isAccepted = (pr: UnifiedPullRequest): boolean =>
  pr.labels.some((label) => label.toLowerCase() === 'accepted');
export const isReviewExcluded = (pr: UnifiedPullRequest): boolean =>
  pr.labels.some((label) => ['blocked', 'deferred'].includes(label.toLowerCase()));
export const hasPendingReviewAction = (pr: PullRequestWorkItem): boolean =>
  pr.reviewStatus === 'outstanding' || !!pr.mentionedAt;

export function isAgedReview(pr: PullRequestWorkItem, now: number): boolean {
  return now - Date.parse(waitingSince(pr) ?? pr.updatedAt) > 30 * 24 * 60 * 60 * 1000;
}

export function waitingSince(pr: PullRequestWorkItem): string | undefined {
  return [pr.reviewStatus === 'outstanding' ? pr.reviewRequestedAt : undefined, pr.mentionedAt]
    .filter((date): date is string => !!date)
    .sort()[0];
}

export function oldestFirst(a: PullRequestWorkItem, b: PullRequestWorkItem): number {
  return a.updatedAt.localeCompare(b.updatedAt) || a.id.localeCompare(b.id);
}

export function waitingFirst(a: PullRequestWorkItem, b: PullRequestWorkItem): number {
  return (
    Number(hasPendingReviewAction(b)) - Number(hasPendingReviewAction(a)) ||
    (waitingSince(a) ?? a.updatedAt).localeCompare(waitingSince(b) ?? b.updatedAt) ||
    oldestFirst(a, b)
  );
}

export function actionFirst(a: PullRequestWorkItem, b: PullRequestWorkItem): number {
  return Number(b.needsAction) - Number(a.needsAction) || oldestFirst(a, b);
}

/** Relative ages are a snapshot of the most recent manual load, not a polling clock. */
export function relativeAge(date: string, now: number): string {
  const minutes = Math.max(0, Math.floor((now - Date.parse(date)) / 60000));
  if (!Number.isFinite(minutes)) return 'unknown';
  if (minutes < 1) return 'just now';
  const [amount, unit] =
    minutes < 60
      ? [minutes, 'minute']
      : minutes < 1440
        ? [Math.floor(minutes / 60), 'hour']
        : [Math.floor(minutes / 1440), 'day'];
  return `${amount} ${unit}${amount === 1 ? '' : 's'}`;
}

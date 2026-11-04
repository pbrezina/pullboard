import { PullRequestWorkItem, UnifiedPullRequest } from './models';

export interface GithubComment {
  id: number;
  user: { login: string } | null;
  body?: string | null;
  created_at: string;
  in_reply_to_id?: number;
}

export interface GithubReview {
  user: { login: string } | null;
  body?: string | null;
  state?: string;
  submitted_at?: string | null;
}

export interface GithubEvent {
  event: string;
  created_at?: string;
  requested_reviewer?: { login: string };
  committer?: { date: string };
}

export interface GithubActivity {
  comments: GithubComment[];
  reviewComments: GithubComment[];
  reviews: GithubReview[];
  timeline: GithubEvent[];
  requested: boolean;
  directlyRequested?: boolean;
  reviewed: boolean;
  createdAt?: string;
}

const sameUser = (a: string | undefined, b: string): boolean =>
  a?.toLowerCase() === b.toLowerCase();
const after = (a: string | undefined, b: string): boolean => !!a && a > b;
const latest = (dates: (string | undefined)[]): string | undefined =>
  dates
    .filter((date): date is string => !!date)
    .sort()
    .at(-1);

function mentions(body: string | null | undefined, username: string): boolean {
  // Match a whole direct login, never a team slug or a longer username.
  const escaped = username.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`(^|[^\\w@/])@${escaped}(?![\\w/-])`, 'i').test(body ?? '');
}

/** REST-only activity heuristics; no notifications are marked read and no replies are posted. */
export function analyzeGithubActivity(
  pr: UnifiedPullRequest,
  username: string,
  activity: GithubActivity,
): PullRequestWorkItem {
  const { comments, reviewComments, reviews, timeline } = activity;
  const ownReview = reviews.some(
    (review) =>
      sameUser(review.user?.login, username) && !!review.submitted_at && review.state !== 'PENDING',
  );
  const requests = timeline.filter(
    (event) =>
      event.event === 'review_requested' && sameUser(event.requested_reviewer?.login, username),
  );
  const requestedAt = latest(requests.map((event) => event.created_at));
  // A re-request remains outstanding even if an older review has already been given.
  const reviewStatus = activity.requested
    ? 'outstanding'
    : ownReview || activity.reviewed
      ? 'given'
      : undefined;

  const generalReplies = [
    ...comments
      .filter((comment) => sameUser(comment.user?.login, username))
      .map((comment) => comment.created_at),
    ...reviews
      .filter((review) => sameUser(review.user?.login, username) && !!review.body?.trim())
      .map((review) => review.submitted_at ?? undefined),
  ];
  const lastGeneralReply = latest(generalReplies);
  const pendingMentions: string[] = [];
  if (
    !sameUser(pr.author, username) &&
    mentions(pr.description, username) &&
    activity.createdAt &&
    !after(lastGeneralReply, activity.createdAt)
  )
    pendingMentions.push(activity.createdAt);
  for (const comment of comments) {
    if (
      !sameUser(comment.user?.login, username) &&
      mentions(comment.body, username) &&
      !after(lastGeneralReply, comment.created_at)
    )
      pendingMentions.push(comment.created_at);
  }
  for (const review of reviews) {
    if (
      review.submitted_at &&
      review.state !== 'DISMISSED' &&
      !sameUser(review.user?.login, username) &&
      mentions(review.body, username) &&
      !after(lastGeneralReply, review.submitted_at)
    )
      pendingMentions.push(review.submitted_at);
  }

  const threads = new Map<number, GithubComment[]>();
  // GitHub's in_reply_to_id points to the root comment of the review thread.
  for (const comment of reviewComments) {
    const root = comment.in_reply_to_id ?? comment.id;
    const thread = threads.get(root) ?? [];
    thread.push(comment);
    threads.set(root, thread);
  }
  const lastCommit = latest(
    timeline.map((event) =>
      event.event === 'committed'
        ? event.committer?.date
        : event.event === 'head_ref_force_pushed'
          ? event.created_at
          : undefined,
    ),
  );
  let needsAction = false;
  const authored = sameUser(pr.author, username);
  for (const thread of threads.values()) {
    const lastReply = latest(
      thread
        .filter((comment) => sameUser(comment.user?.login, username))
        .map((comment) => comment.created_at),
    );
    for (const comment of thread) {
      if (sameUser(comment.user?.login, username)) continue;
      if (mentions(comment.body, username) && !after(lastReply, comment.created_at))
        pendingMentions.push(comment.created_at);
      if (
        authored &&
        comment.body?.trim() &&
        !after(lastReply, comment.created_at) &&
        !after(lastCommit, comment.created_at)
      )
        needsAction = true;
    }
  }
  const decisions = new Map<string, GithubReview>();
  for (const review of [...reviews].sort((a, b) =>
    (a.submitted_at ?? '').localeCompare(b.submitted_at ?? ''),
  )) {
    if (review.user && review.submitted_at && review.state !== 'PENDING') {
      if (['CHANGES_REQUESTED', 'APPROVED', 'DISMISSED'].includes(review.state ?? ''))
        decisions.set(review.user.login.toLowerCase(), review);
    }
  }
  if (authored) {
    for (const comment of comments) {
      if (
        !sameUser(comment.user?.login, username) &&
        comment.body?.trim() &&
        !after(lastGeneralReply, comment.created_at) &&
        !after(lastCommit, comment.created_at)
      )
        needsAction = true;
    }
    for (const review of reviews) {
      if (sameUser(review.user?.login, username) || !review.submitted_at) continue;
      const decision = review.user ? decisions.get(review.user.login.toLowerCase()) : undefined;
      // A later approval supersedes earlier summary feedback, but an empty COMMENTED
      // review (often just an inline reply) does not silently dismiss it.
      if (
        decision?.state === 'APPROVED' &&
        after(decision.submitted_at ?? undefined, review.submitted_at)
      )
        continue;
      if (review.state === 'CHANGES_REQUESTED' && decision !== review) continue;
      if (
        (review.state === 'CHANGES_REQUESTED' ||
          (review.state === 'COMMENTED' && !!review.body?.trim())) &&
        !after(lastGeneralReply, review.submitted_at) &&
        !after(lastCommit, review.submitted_at)
      )
        needsAction = true;
    }
  }
  return {
    ...pr,
    reviewStatus,
    // Never invent an old waiting age when the request timestamp is unavailable.
    reviewRequestedAt:
      reviewStatus === 'outstanding' && activity.directlyRequested !== false
        ? requestedAt
        : undefined,
    mentionedAt: pendingMentions.sort()[0],
    needsAction,
  };
}

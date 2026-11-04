import { analyzeGithubActivity, GithubActivity, GithubComment } from './github-activity';
import { testPr } from './testing';

describe('GitHub activity classification', () => {
  const monday = '2026-11-02T10:00:00Z';
  const tuesday = '2026-11-03T10:00:00Z';
  const wednesday = '2026-11-04T10:00:00Z';
  const empty: GithubActivity = {
    comments: [],
    reviewComments: [],
    reviews: [],
    timeline: [],
    requested: false,
    reviewed: false,
  };
  const comment = (
    id: number,
    login: string,
    body: string,
    date = monday,
    root?: number,
  ): GithubComment => ({
    id,
    user: { login },
    body,
    created_at: date,
    in_reply_to_id: root,
  });
  const otherPr = { ...testPr, author: 'colleague' };

  it('retains past reviews and distinguishes a later re-request', () => {
    const activity: GithubActivity = {
      ...empty,
      reviews: [{ user: { login: 'DEV' }, state: 'APPROVED', submitted_at: monday }],
      timeline: [
        { event: 'review_requested', requested_reviewer: { login: 'dev' }, created_at: monday },
        { event: 'review_requested', requested_reviewer: { login: 'DEV' }, created_at: wednesday },
      ],
    };
    expect(analyzeGithubActivity(otherPr, 'dev', activity).reviewStatus).toBe('given');
    const requested = analyzeGithubActivity(otherPr, 'dev', { ...activity, requested: true });
    expect(requested.reviewStatus).toBe('outstanding');
    expect(requested.reviewRequestedAt).toBe(wednesday);
  });

  it('does not invent request dates or treat pending reviews as submitted', () => {
    const result = analyzeGithubActivity(otherPr, 'dev', {
      ...empty,
      requested: true,
      reviews: [{ user: { login: 'dev' }, state: 'PENDING', submitted_at: null }],
    });
    expect(result.reviewStatus).toBe('outstanding');
    expect(result.reviewRequestedAt).toBeUndefined();
  });

  it('keeps mentions unanswered when a reply is in a different review thread', () => {
    const result = analyzeGithubActivity(otherPr, 'dev', {
      ...empty,
      comments: [comment(10, 'dev', 'General reply', wednesday)],
      reviewComments: [
        comment(1, 'colleague', '@dev please check'),
        comment(2, 'colleague', 'Another thread'),
        comment(3, 'DEV', 'Replied elsewhere', tuesday, 2),
      ],
    });
    expect(result.mentionedAt).toBe(monday);
  });

  it('clears mentions only after a same-thread reply, and detects a new mention', () => {
    const comments = [
      comment(1, 'colleague', '@dev please check'),
      comment(2, 'DEV', 'Done', tuesday, 1),
    ];
    expect(
      analyzeGithubActivity(otherPr, 'dev', { ...empty, reviewComments: comments }).mentionedAt,
    ).toBeUndefined();
    expect(
      analyzeGithubActivity(otherPr, 'dev', {
        ...empty,
        reviewComments: [
          ...comments,
          comment(3, 'colleague', '@dev another question', wednesday, 1),
        ],
      }).mentionedAt,
    ).toBe(wednesday);
  });

  it('treats the PR conversation as a thread, independently of inline replies', () => {
    const activity = {
      ...empty,
      comments: [comment(1, 'colleague', '@dev question')],
      reviewComments: [comment(2, 'dev', 'Inline reply', tuesday, 99)],
    };
    expect(analyzeGithubActivity(otherPr, 'dev', activity).mentionedAt).toBe(monday);
    expect(
      analyzeGithubActivity(otherPr, 'dev', {
        ...activity,
        comments: [...activity.comments, comment(3, 'dev', 'General answer', tuesday)],
      }).mentionedAt,
    ).toBeUndefined();
  });

  it.each(['@developer', 'mail@dev.example', '@dev/team', '@org/dev', '@dev-other'])(
    'does not mistake %s for a direct mention',
    (body) => {
      expect(
        analyzeGithubActivity(otherPr, 'dev', {
          ...empty,
          comments: [comment(1, 'colleague', body)],
        }).mentionedAt,
      ).toBeUndefined();
    },
  );

  it('includes body and review-summary mentions and selects the oldest unanswered one', () => {
    const result = analyzeGithubActivity({ ...otherPr, description: 'Please @DeV!' }, 'dev', {
      ...empty,
      createdAt: monday,
      reviews: [
        {
          user: { login: 'colleague' },
          body: '@dev more?',
          state: 'COMMENTED',
          submitted_at: tuesday,
        },
      ],
    });
    expect(result.mentionedAt).toBe(monday);
  });

  it('ignores self-mentions', () => {
    expect(
      analyzeGithubActivity(testPr, 'dev', {
        ...empty,
        comments: [comment(1, 'dev', '@dev note')],
      }).mentionedAt,
    ).toBeUndefined();
  });

  it('highlights changes requested until a reply, new commit, or approval', () => {
    const activity: GithubActivity = {
      ...empty,
      reviews: [{ user: { login: 'reviewer' }, state: 'CHANGES_REQUESTED', submitted_at: monday }],
    };
    expect(analyzeGithubActivity(testPr, 'dev', activity).needsAction).toBe(true);
    expect(
      analyzeGithubActivity(testPr, 'dev', {
        ...activity,
        comments: [comment(1, 'DEV', 'Fixed', tuesday)],
      }).needsAction,
    ).toBe(false);
    expect(
      analyzeGithubActivity(testPr, 'dev', {
        ...activity,
        timeline: [{ event: 'committed', committer: { date: tuesday } }],
      }).needsAction,
    ).toBe(false);
    expect(
      analyzeGithubActivity(testPr, 'dev', {
        ...activity,
        reviews: [
          ...activity.reviews,
          { user: { login: 'reviewer' }, state: 'APPROVED', submitted_at: tuesday },
        ],
      }).needsAction,
    ).toBe(false);
  });

  it('does not clear a change request just because a later empty comment review exists', () => {
    expect(
      analyzeGithubActivity(testPr, 'dev', {
        ...empty,
        reviews: [
          { user: { login: 'reviewer' }, state: 'CHANGES_REQUESTED', submitted_at: monday },
          { user: { login: 'reviewer' }, state: 'COMMENTED', submitted_at: tuesday },
        ],
      }).needsAction,
    ).toBe(true);
  });

  it('keeps earlier unanswered summary feedback after an empty inline-comment review', () => {
    expect(
      analyzeGithubActivity(testPr, 'dev', {
        ...empty,
        reviews: [
          {
            user: { login: 'reviewer' },
            state: 'COMMENTED',
            body: 'Please update docs',
            submitted_at: monday,
          },
          { user: { login: 'reviewer' }, state: 'COMMENTED', body: '', submitted_at: tuesday },
        ],
      }).needsAction,
    ).toBe(true);
  });

  it('does not reuse a historical personal request timestamp for a current team-only request', () => {
    expect(
      analyzeGithubActivity(otherPr, 'dev', {
        ...empty,
        requested: true,
        directlyRequested: false,
        timeline: [
          { event: 'review_requested', requested_reviewer: { login: 'dev' }, created_at: monday },
        ],
      }).reviewRequestedAt,
    ).toBeUndefined();
  });

  it('requires same-thread replies for inline feedback; a new commit can clear all feedback', () => {
    const activity: GithubActivity = {
      ...empty,
      reviewComments: [
        comment(1, 'reviewer', 'Please fix'),
        comment(2, 'dev', 'Other reply', tuesday, 99),
      ],
      comments: [comment(3, 'dev', 'General reply', tuesday)],
    };
    expect(analyzeGithubActivity(testPr, 'dev', activity).needsAction).toBe(true);
    expect(
      analyzeGithubActivity(testPr, 'dev', {
        ...activity,
        reviewComments: [...activity.reviewComments, comment(4, 'dev', 'Fixed', wednesday, 1)],
      }).needsAction,
    ).toBe(false);
    expect(
      analyzeGithubActivity(testPr, 'dev', {
        ...activity,
        timeline: [{ event: 'head_ref_force_pushed', created_at: wednesday }],
      }).needsAction,
    ).toBe(false);
  });

  it('does not highlight an approval, a dismissed request, or feedback on someone else’s PR', () => {
    for (const state of ['APPROVED', 'DISMISSED']) {
      expect(
        analyzeGithubActivity(testPr, 'dev', {
          ...empty,
          reviews: [{ user: { login: 'reviewer' }, state, submitted_at: monday }],
        }).needsAction,
      ).toBe(false);
    }
    expect(
      analyzeGithubActivity(otherPr, 'dev', {
        ...empty,
        reviewComments: [comment(1, 'reviewer', 'Fix this')],
      }).needsAction,
    ).toBe(false);
  });

  it('highlights unanswered general conversation feedback', () => {
    expect(
      analyzeGithubActivity(testPr, 'dev', {
        ...empty,
        comments: [comment(1, 'reviewer', 'Please update the docs')],
      }).needsAction,
    ).toBe(true);
  });
});

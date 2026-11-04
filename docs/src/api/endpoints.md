# Remote Endpoints

All browser API paths are relative to the local proxy prefixes. Credentials
are never interpolated into URLs. There are no application backend endpoints.

## GitHub (GET only)

| Path under `/api/forges/<index>`                 | Purpose                                                                                                                                                        |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/user`                                          | Resolve authenticated login.                                                                                                                                   |
| `/search/issues`                                 | Paginated open, non-archived PR searches: `author:<login>`; the PR inbox also unions `review-requested:<login>`, `reviewed-by:<login>` and `mentions:<login>`. |
| `/repos/{owner}/{repo}/pulls/{number}`           | Live state, draft flag, merged timestamp, assignees and requested reviewers/teams.                                                                             |
| `/repos/{owner}/{repo}/pulls/{number}/reviews`   | Paginated reviewers, review states, bodies and submission times.                                                                                               |
| `/repos/{owner}/{repo}/pulls/{number}/comments`  | Paginated inline review comments and same-thread reply IDs (PR inbox).                                                                                         |
| `/repos/{owner}/{repo}/issues/{number}/comments` | Paginated general PR conversation comments (PR inbox).                                                                                                         |
| `/repos/{owner}/{repo}/issues/{number}/timeline` | Paginated review request, commit and force-push activity (PR inbox).                                                                                           |

The provider uses JSON Accept, Bearer authorization and API version 2022-11-28.

## JIRA

| Remote path                                                | Method     | Purpose                                                                                                                                   |
| ---------------------------------------------------------- | ---------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `/rest/api/3/myself`                                       | GET        | Verify current account.                                                                                                                   |
| `/rest/api/3/user/search`                                  | GET        | Resolve configured email to account ID.                                                                                                   |
| `/rest/agile/1.0/board`                                    | GET        | Project Scrum boards.                                                                                                                     |
| `/rest/agile/1.0/board/{id}/sprint`                        | GET        | Active and future sprints (`state=active,future`).                                                                                        |
| `/rest/agile/1.0/sprint/{id}`                              | GET        | Check selected sprint is still active or future.                                                                                          |
| `/rest/api/3/search/jql`                                   | GET        | Token-paginated unfinished project/assignee search by sprint or `sprint IS EMPTY`. No project-wide Web Link scan.                         |
| `/rest/api/3/issue/createmeta/{project}/issuetypes`        | GET        | Creatable issue types.                                                                                                                    |
| `/rest/api/3/issue/createmeta/{project}/issuetypes/{type}` | GET        | Create-screen fields.                                                                                                                     |
| `/rest/api/3/issue`                                        | POST       | Create assigned implementation issue or explicitly unassigned review issue with ADF description. Review issues retain the default status. |
| `/rest/agile/1.0/sprint/{id}/issue`                        | POST       | Opt-in assignment to the chosen active/future sprint.                                                                                     |
| `/rest/api/3/issue/{key}/remotelink`                       | GET / POST | Inspect or upsert Web Links.                                                                                                              |
| `/rest/api/3/issue/{key}`                                  | GET        | Current workflow status.                                                                                                                  |
| `/rest/api/3/issue/{key}/transitions`                      | GET / POST | Discover then execute available transition.                                                                                               |

JIRA Cloud uses REST API v3 and Agile API 1.0. The removed legacy `/search`
endpoint and global hard-coded customfield/transition IDs are not used.
Test mode reads real data but simulates all JIRA mutations. Both the browser
interceptor and test-mode dev proxy reject non-GET JIRA requests; no creation,
sprint, link or transition POST reaches JIRA. AI POST requests are unaffected.

## LLM

- OpenAI: `POST /api/openai/v1/chat/completions`, JSON object response format.
- Gemini: `POST /api/gemini/v1beta/models/{model}:generateContent`, JSON MIME type.

Both prompts ask for `{ "title": string, "description": string,
"storyPoints": number }`. Output is validated before showing the dialog.

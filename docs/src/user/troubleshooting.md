# Troubleshooting

| Symptom                                | Action                                                                                                                                                                                              |
| -------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Config cannot load                     | Create `public/assets/config.json`, validate its JSON, restart `npm start`.                                                                                                                         |
| Proxy connection failure               | Regenerate the proxy after target edits; check DNS, VPN and trusted TLS certificates.                                                                                                               |
| 401/403                                | Check token expiry, read scopes, organization SSO, JIRA permissions and correct tenant.                                                                                                             |
| Sprint unavailable                     | Set `jira.boardId` for the intended Scrum board. Use `jira.sprintNamePattern` or `jira.sprintId` to identify the active default; otherwise select a listed future sprint or use No sprint assigned. |
| No PRs                                 | Search uses the authenticated token's user, not the JIRA email. Check private repository access and API warnings.                                                                                   |
| Search incomplete                      | GitHub search has a 1,000-result limit; Pullboard refuses a silently truncated list.                                                                                                                |
| Story points error                     | Expose a numeric estimation field on the create screen, or configure its field ID.                                                                                                                  |
| Additional required fields             | Set JIRA defaults or use the issue directly in JIRA; the dialog supports title, description and points only.                                                                                        |
| No available transition                | Configure destination names or update the workflow. Pullboard does not take arbitrary intermediate transitions.                                                                                     |
| Transition requires fields             | Complete it in JIRA or configure workflow defaults.                                                                                                                                                 |
| LLM model not found                    | Choose a currently served model in `llm.model`; legacy Gemini defaults may be retired.                                                                                                              |
| Rate limit                             | Wait, then use **Refresh board**. There is no automatic polling; review/thread history uses more reads than the JIRA view. Activity and status checks have bounded concurrency.                     |
| Partial sync                           | Use the existing large JIRA link and Resume sync; do not create a duplicate issue.                                                                                                                  |
| Partial review ticket                  | Use **Finish review ticket** to retry its Web Link and optional sprint on the saved ticket. Do not create another.                                                                                  |
| Review ticket cannot be unassigned     | Ask a JIRA administrator to allow unassigned issues; Pullboard explicitly sends a null assignee rather than choosing the project default.                                                           |
| Missing PR-card JIRA links             | Only local receipts and already-loaded sprint tickets supply links. Project-wide scanning is disabled; check JIRA manually for other tickets.                                                       |
| Older partial receipt has no sprint ID | Complete the existing ticket's sprint, Web Link and status in JIRA; Pullboard will not guess a new sprint.                                                                                          |
| No matching cards                      | Clear the column's title filter; for JIRA also verify the sprint/no-sprint selection and assignee.                                                                                                  |
| Actions say simulated                  | The server was started with `npm run start:test`. Stop it and use `npm start` for real JIRA writes.                                                                                                 |
| Creation outcome unknown               | Check recent JIRA tickets before allowing a new attempt. If created, complete its sprint, Web Link and status in JIRA instead of creating another.                                                  |

Errors are inline, not snackbars, and do not display raw HTTP URLs/response
bodies that might expose credentials. An API failure does not mean a PR closed.
Forge failures appear as warnings; JIRA failures disable new mutation actions
until a successful refresh. Previously loaded cards can be stale.

## JIRA XSRF errors

If the response body says `XSRF check failed`, stop the development server and
run `npm start` again to regenerate and reload the proxy's JIRA Origin/Referer
overrides, then use **Resume sync**. The plain-text error can also appear as a
`JSON.parse` error in browser tools; it does not mean the request JSON is malformed.
See [API Proxy Setup](reverse-proxy.md) for the required headers.

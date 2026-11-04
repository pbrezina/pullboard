# Authentication and Permissions

Pullboard does not implement OAuth. Credentials are loaded from the ignored
local config over the loopback development server.

## GitHub

Prefer a fine-grained token restricted to the repositories you need, with
read-only Metadata, Pull requests and Issues access. The PR workbench reads issue
conversation comments and timelines in addition to PR reviews and inline review
comments. Restricted permissions produce visible incomplete-activity warnings.
Never grant write permissions.
Private repository search also depends on organization/SSO authorization.
Classic tokens may have broader scopes; prefer read-only fine-grained tokens
where supported. No Pullboard operation needs upstream writes.

## JIRA Cloud

Use your Atlassian email and API token. The client sends HTTP Basic auth through
the local JIRA proxy. This setup targets tenant-domain JIRA REST access with
a compatible API token; scoped tokens requiring Atlassian gateway URLs are not
interchangeable with this tenant-domain configuration.

You need appropriate product access and permissions to browse users, projects,
boards/sprints and issues; create, assign, link, schedule and transition issues.
The user search must resolve your email (or the authenticated account) so
account IDs can be verified. Privacy restrictions may require admin assistance.
There is no project-wide PR-card link discovery. Known links come from local
receipts and the tickets already loaded in the synchronization view. Review
tickets are explicitly unassigned; the project must allow unassigned issues.
An available workflow transition can still require a permission or extra field;
Pullboard surfaces that failure instead of pretending the operation succeeded.

## LLM

Use a Gemini API key or OpenAI API key with access to your chosen model and
available quota. Verify your organization's policy before sending private PR
text. Keys go in headers, not query parameters. Model output is untrusted,
validated JSON, rendered as text, and reviewed by the user before any creation.

## Local security

Git ignores `public/assets/config.json` and generated `proxy.conf.json`.
Do not commit them or share browser network exports containing credentials.
The production build excludes the config asset; development builds include it.
The browser and local proxy can read secrets; loopback binding is essential.
No remote hosting, multi-user isolation, or at-rest credential vault is provided.
Only JIRA receipts (PR URL, JIRA key/link, sync state) are stored in localStorage,
not tokens or PR descriptions. Clear browser site data to remove receipts.

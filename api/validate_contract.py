#!/usr/bin/env python3
"""Offline contract gate; requires jsonschema and openapi-spec-validator.

Validates schemas/examples, not current server responses or runtime concurrency.
Run from any directory: python /path/to/api/validate_contract.py
"""

import copy
import json
import re
from pathlib import Path

from jsonschema import Draft202012Validator, FormatChecker, ValidationError
from openapi_spec_validator import validate


BASE = Path(__file__).resolve().parent


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"Duplicate JSON key: {key}")
        result[key] = value
    return result


documents = {
    path.relative_to(BASE).as_posix(): json.loads(
        path.read_text(), object_pairs_hook=unique_object
    )
    for path in BASE.rglob("*.json")
}
spec = documents["openapi.json"]
contract = (BASE / "CONTRACT.md").read_text()
if f"v{spec['info']['version']} " not in contract.splitlines()[0]:
    raise ValueError("Normative prose/OpenAPI versions differ")


def walk(value):
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from walk(child)
    elif isinstance(value, list):
        for child in value:
            yield from walk(child)


def resolve(ref):
    if not ref.startswith("#/"):
        raise ValueError(f"Non-self-contained reference: {ref}")
    value = spec
    for part in ref[2:].split("/"):
        value = value[part.replace("~1", "/").replace("~0", "~")]
    return value


def progress_invariants(value):
    total = value["total_items"]
    if total is not None and value["completed_items"] > total:
        raise ValueError("completed_items exceeds the known total")
    if value["phase"] in {"publishing", "completed"}:
        if total != value["completed_items"]:
            raise ValueError("Publishing/completed must have fully fetched totals")


def options_progress_invariants(value):
    for collection in ("groups", "projects"):
        rows = value[collection]
        names = [row["name"] for row in rows]
        if len(set(names)) != len(names):
            raise ValueError("Options work row names must be unique within each domain")
        for row in rows:
            progress_invariants(row)
            if row["completed_items"] and row["total_items"] is None:
                raise ValueError("Eligibility checks require a complete known workload")
    if value["phase"] in {"publishing", "completed"} and not value["projects"]:
        raise ValueError("Complete options require all configured projects (nonempty in YAML)")


def attempt_invariants(health, progress):
    phases = {"completed": "ok", "failed": "error", "backoff": "backoff"}
    phase = progress["phase"]
    if phase in phases and health["status"] != phases[phase]:
        raise ValueError("Terminal progress must match its own domain health")
    if progress["started_at"] is not None:
        if health["last_attempt_at"] != progress["started_at"]:
            raise ValueError("Live attempt timestamp must match its own domain progress")
    if phase == "completed" and health["last_success_at"] != progress["finished_at"]:
        raise ValueError("Domain success timestamp must match its own committed completion")


def options_status_invariants(value):
    options_progress_invariants(value["progress"])
    attempt_invariants(value["health"], value["progress"])


def sync_invariants(value):
    sources = value["sources"]
    progress = value["progress"]
    ids = [source["source_id"] for source in sources]
    if len(set(ids)) != len(ids) or ids != [p["source_id"] for p in progress]:
        raise ValueError("Sources/progress must have identical unique ordered IDs")
    jira = [source for source in sources if source["kind"] == "jira"]
    if len(jira) > 1 or any((source["source_id"] == "jira") != (source["kind"] == "jira") for source in sources):
        raise ValueError("Jira is one configured source with the reserved source_id jira")
    for source, entry in zip(sources, progress):
        progress_invariants(entry)
        phase = entry["phase"]
        if value["state"] == "idle" and phase in {
            "queued", "preparing", "discovering", "fetching", "publishing"
        }:
            raise ValueError("Idle job cannot have queued/active progress")
        attempt_invariants(source, entry)
    options = value["jira_options"]
    if options is not None:
        options_status_invariants(options)
        options_phase = options["progress"]["phase"]
        card_fetching = {"idle", "queued", "preparing", "discovering", "fetching"}
        jira_progress = next(entry for entry in progress if entry["source_id"] == "jira")
        if options_phase in {"preparing", "discovering", "fetching", "publishing", "completed"}:
            if jira_progress["phase"] in card_fetching:
                raise ValueError("Creation discovery follows Jira card fetching")
        if options_phase in {"fetching", "publishing", "completed"}:
            if any(entry["phase"] in card_fetching for entry in progress):
                raise ValueError("Final eligibility follows all source card fetching")
    if value["state"] == "idle" and value["finished_at"] is not None:
        required_phases = [entry["phase"] for entry in progress]
        if options is not None:
            required_phases.append(options["progress"]["phase"])
        succeeded = all(phase == "completed" for phase in required_phases)
        if succeeded != (value["last_success_at"] == value["finished_at"]):
            raise ValueError("Job success requires every ticket and options domain to succeed")


def board_invariants(value):
    sources = {source["source_id"]: source for source in value["sources"]}
    if len(sources) != len(value["sources"]):
        raise ValueError("Board source IDs must be unique")
    ids = set()
    pr_ids = {pr["id"] for pr in value["pull_requests"]}
    for issue in value["upstream_issues"]:
        if issue["id"] in ids or issue["id"] in pr_ids:
            raise ValueError("Upstream issue IDs must be unique and separate from PR IDs")
        ids.add(issue["id"])
        source = sources.get(issue["source_id"])
        if source is None or source["kind"] != issue["forge"]:
            raise ValueError("Upstream issues must match a board forge instance/kind")


def sorted_unique_choices(choices):
    ids = [choice["id"] for choice in choices]
    if len(set(ids)) != len(ids):
        raise ValueError("Choice IDs must be unique")
    if choices != sorted(choices, key=lambda choice: (choice["name"].casefold(), choice["id"])):
        raise ValueError("Choices must sort by case-insensitive name then opaque ID")


def context_invariants(value):
    projects = value["jira_projects"]
    keys = [project["key"] for project in projects]
    if len(set(keys)) != len(keys):
        raise ValueError("Configured discovered projects must be unique")
    current = value["current_project"]
    if current is not None and (not projects or current != projects[0]):
        raise ValueError("Current project must be the discovered first configured project")
    sorted_unique_choices(value["current_sprints"])


def options_invariants(value):
    sorted_unique_choices(value["statuses"])
    ids = [assignee["id"] for assignee in value["assignees"]]
    if len(set(ids)) != len(ids):
        raise ValueError("Eligible assignees must be deduplicated")
    if value["issue_type"] is not None and value["issue_type"] not in value["issue_types"]:
        raise ValueError("Selected issue type must belong to the cached inventory")


def validate_value(schema, value):
    wrapper = {
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "components": spec["components"],
        **schema,
    }
    Draft202012Validator(wrapper, format_checker=FormatChecker()).validate(value)
    ref = schema.get("$ref", "")
    if ref.endswith(("/SourceProgress", "/ChoiceProgress")):
        progress_invariants(value)
    elif ref.endswith("/JiraOptionsProgress"):
        options_progress_invariants(value)
    elif ref.endswith("/JiraOptionsStatus"):
        options_status_invariants(value)
    elif ref.endswith("/SyncStatus"):
        sync_invariants(value)
    elif ref.endswith("/Board"):
        board_invariants(value)
    elif ref.endswith("/Context"):
        context_invariants(value)
    elif ref.endswith("/JiraOptions"):
        options_invariants(value)


def schema_for(name):
    return {"$ref": f"#/components/schemas/{name}"}


refs = 0
for node in walk(spec):
    if "$ref" in node:
        resolve(node["$ref"])
        refs += 1
validate(spec)

examples = 0
checked_files = set()
for name, schema in spec["components"]["schemas"].items():
    Draft202012Validator.check_schema(schema)
    for example in schema.get("examples", []):
        validate_value(schema_for(name), example)
        examples += 1

for node in walk(spec):
    if "schema" not in node:
        continue
    if "example" in node:
        validate_value(node["schema"], node["example"])
        examples += 1
    for example in node.get("examples", {}).values():
        if "$ref" in example:
            example = resolve(example["$ref"])
        if "externalValue" in example:
            path = example["externalValue"]
            if path not in documents or not path.startswith("examples/"):
                raise ValueError(f"Example must be a bundled JSON fixture: {path}")
            value = documents[path]
            checked_files.add(path)
        else:
            value = example["value"]
        validate_value(node["schema"], value)
        examples += 1

for path, value in documents.items():
    if path.startswith("examples/options-"):
        validate_value(schema_for("JiraOptions"), value)
        checked_files.add(path)
        examples += 1
if checked_files != set(documents) - {"openapi.json"}:
    raise ValueError("Every bundled JSON fixture must be validated against a schema")

for text in re.findall(r"```json\n(.*?)\n```", contract, flags=re.DOTALL):
    value = json.loads(text, object_pairs_hook=unique_object)
    name = "CreateIssueRequest" if "purpose" in value else "CreateIssueResult"
    validate_value(schema_for(name), value)
    examples += 1

negative_cases = []


def reject(name, value):
    negative_cases.append((name, value))


schemas = spec["components"]["schemas"]
for name in (
    "CreateIssueRequest", "CreateUpstreamIssueRequest", "LinkRequest",
    "ChangeRuntimeModeRequest",
):
    original = schemas[name]["examples"][0]
    missing = copy.deepcopy(original)
    del missing["expected_mode_revision"]
    reject(name, missing)
    for invalid in (None, "", "x" * 129, 1):
        reject(name, {**original, "expected_mode_revision": invalid})
    reject(name, {**original, "unexpected": True})

# Every new object is closed and every declared field (including nullable fields)
# is required. Exercise missing/unknown fields independently of mode extraction.
new_examples = {
    "SyncStatus": documents["examples/sync-completed.json"],
    "JiraOptionsStatus": documents["examples/sync-completed.json"]["jira_options"],
    "JiraOptionsProgress": documents["examples/sync-completed.json"]["jira_options"]["progress"],
    "ChoiceProgress": schemas["ChoiceProgress"]["examples"][0],
    "Context": documents["examples/context-current.json"],
    "JiraOptions": documents["examples/options-implementation.json"],
    "JiraDefaults": documents["examples/options-implementation.json"]["defaults"],
    "JiraAssignee": documents["examples/options-implementation.json"]["assignees"][0],
    "StatusChoice": documents["examples/options-implementation.json"]["statuses"][0],
    "UpstreamIssue": documents["examples/upstream-resolved-closed.json"]["issue"],
    "ResolveUpstreamIssueRequest": schemas["ResolveUpstreamIssueRequest"]["examples"][0],
    "ResolvedUpstreamIssue": documents["examples/upstream-resolved-closed.json"],
    "UpstreamIssueDraftRequest": schemas["UpstreamIssueDraftRequest"]["examples"][0],
    "UpstreamIssueDraft": documents["examples/upstream-draft-closed.json"],
    "CreateUpstreamIssueRequest": documents["examples/upstream-create-request.json"],
}
for name, original in new_examples.items():
    schema = schemas[name]
    if schema.get("additionalProperties") is not False:
        raise ValueError(f"New schema must be strict: {name}")
    if set(schema["required"]) != set(schema["properties"]):
        raise ValueError(f"Every new schema field must be required: {name}")
    reject(name, {**original, "unexpected": True})
    for field in schema["required"]:
        reject(name, {key: value for key, value in original.items() if key != field})

# Shared editor fields must not fork the existing Jira wire contract. Only the
# trusted selector and fixed implementation purpose differ in the new request.
pr_fields = schemas["CreateIssueRequest"]["properties"]
upstream_fields = schemas["CreateUpstreamIssueRequest"]["properties"]
if set(upstream_fields) != ((set(pr_fields) - {"pull_request_id"}) | {"upstream_issue_id"}):
    raise ValueError("Upstream create must reuse the existing editor field inventory")


def without_description(value):
    return {key: item for key, item in value.items() if key != "description"}


for field in set(pr_fields) - {"pull_request_id", "purpose"}:
    # Human descriptions may differ; the validation keywords must remain identical.
    if without_description(pr_fields[field]) != without_description(upstream_fields[field]):
        raise ValueError(f"Upstream/PR create field drift: {field}")
if without_description(pr_fields["status"]) != schema_for("NullableString"):
    raise ValueError("Create status must preserve the existing nullable name string wire type")
paths = spec["paths"]
for path in (
    "/api/upstream-issues/resolve", "/api/upstream-issues/ticket-drafts",
    "/api/upstream-issues/jira-issues",
):
    if {"$ref": "#/components/parameters/WriteHeader"} not in paths[path]["post"]["parameters"]:
        raise ValueError("New POSTs must retain the shared request security header")
create_responses = paths["/api/upstream-issues/jira-issues"]["post"]["responses"]
if create_responses["200"]["content"]["application/json"]["schema"] != schema_for("CreateIssueResult"):
    raise ValueError("Upstream create must reuse CreateIssueResult")
if create_responses["409"] != {"$ref": "#/components/responses/ModeConflict"}:
    raise ValueError("Upstream create must retain mode-conflict admission")

board = documents["examples/board-upstream-issues.json"]
if {issue["forge"] for issue in board["upstream_issues"]} != set(schemas["ForgeKind"]["enum"]):
    raise ValueError("Assigned board fixture must cover all four forge kinds")
empty_board = {**board, "revision": 0, "generated_at": None, "upstream_issues": [], "sources": []}
validate_value(schema_for("Board"), empty_board)
examples += 1
reject("Board", {key: value for key, value in board.items() if key != "upstream_issues"})
for invalid in (
    {**board["upstream_issues"][0], "state": "closed"},
    {**board["upstream_issues"][0], "is_assigned_to_me": False},
    {**board["upstream_issues"][0], "source_id": "unconfigured"},
    {**board["upstream_issues"][0], "forge": "github"},
):
    reject("Board", {**board, "upstream_issues": [invalid]})
reject("Board", {**board, "upstream_issues": [board["upstream_issues"][0]] * 2})

resolved = new_examples["ResolvedUpstreamIssue"]
manual = resolved["issue"]
draft = new_examples["UpstreamIssueDraft"]
create = new_examples["CreateUpstreamIssueRequest"]
if manual["id"] in {issue["id"] for issue in board["upstream_issues"]}:
    raise ValueError("Manual closed/unassigned fixture must not enter Board")
if not (manual["state"] == "closed" and not manual["is_assigned_to_me"]):
    raise ValueError("Manual flow must exercise a closed unassigned issue")
expected_description = (
    f"{manual['description']}\n\nUpstream issue: {manual['url']}\n"
    f"Labels: {', '.join(manual['labels'])}"
)
if not (
    draft["upstream_issue_id"] == create["upstream_issue_id"] == manual["id"]
    and draft["title"] == create["title"] == manual["title"]
    and draft["description"] == create["description"] == expected_description
    and draft["warning"] == resolved["warning"]
):
    raise ValueError("Manual resolve/draft/create fixtures must preserve trusted identity/text/warning")
for invalid in (None, "", " \t\n"):
    reject("ResolvedUpstreamIssue", {**resolved, "warning": invalid})
for invalid in ("", " \t\n"):
    reject("UpstreamIssueDraft", {**draft, "warning": invalid})
open_resolution = {"issue": board["upstream_issues"][0], "warning": None}
validate_value(schema_for("ResolvedUpstreamIssue"), open_resolution)
examples += 1
reject("ResolvedUpstreamIssue", {**open_resolution, "warning": "Not closed"})
for name in ("UpstreamIssueDraft", "CreateUpstreamIssueRequest"):
    reject(name, {**new_examples[name], "purpose": "review"})
    reject(name, {**new_examples[name], "pull_request_id": "not-an-upstream-issue"})
reject("UpstreamIssueDraft", {**draft, "generated_by": "ai"})
reject("UpstreamIssueDraft", {**draft, "suggested_status": "Ready"})
reject("CreateUpstreamIssueRequest", {**create, "title": "x" * 256})
reject("CreateUpstreamIssueRequest", {**create, "description": "x" * 100001})
reject("CreateUpstreamIssueRequest", {**create, "story_points": -1})
reject("CreateIssueRequest", {**schemas["CreateIssueRequest"]["examples"][0], "upstream_issue_id": manual["id"]})
reject("DraftRequest", {
    "pull_request_id": "work-github:acme/widget:42", "purpose": "implementation",
    "project_key": None, "upstream_issue_id": manual["id"],
})
reject("LinkRequest", {**schemas["LinkRequest"]["examples"][0], "upstream_issue_id": manual["id"]})

# Current project is discovered/configured, not a hardcoded key; only matching
# active sprints enter context while board/options retain every board sprint.
context = new_examples["Context"]
options = new_examples["JiraOptions"]
expected_current = sorted(
    [sprint for sprint in options["sprints"] if sprint["default_eligible"]],
    key=lambda sprint: (sprint["name"].casefold(), sprint["id"]),
)
if context["current_sprints"] != expected_current:
    raise ValueError("Context fixture must retain only matching active board sprints")
alternate_project = {**context["current_project"], "key": "EXAMPLE", "name": "Another project"}
validate_value(schema_for("Context"), {
    **context, "current_project": alternate_project, "jira_projects": [alternate_project],
})
validate_value(schema_for("Context"), {**context, "current_project": None, "current_sprints": []})
examples += 2
reject("Context", {**context, "current_project": context["jira_projects"][1]})
reject("Context", {**context, "current_sprints": [options["sprints"][0]]})
reject("Context", {**context, "current_sprints": [options["sprints"][2]]})
reject("Context", {**context, "current_sprints": context["current_sprints"] * 2})
# Keep all matching candidates for an ambiguous header/explicit Overview choice.
# Array order is display-only: zero/multiple matches must not auto-select a sprint.
# UI initial/reset behavior is normative prose, not a new wire selection field.
multiple = documents["examples/options-review-ambiguous.json"]["sprints"]
validate_value(schema_for("Context"), {**context, "current_sprints": multiple})
examples += 1
reject("Context", {**context, "current_sprints": list(reversed(multiple))})
reject("JiraOptions", {**options, "statuses": options["statuses"] * 2})
reject("JiraOptions", {**options, "statuses": list(reversed(options["statuses"]))})
reject("JiraOptions", {**options, "assignees": options["assignees"] * 2})
reject("JiraOptions", {**options, "issue_type": "Unknown"})
reject("JiraOptions", {**options, "issue_type": None})
no_type = {
    **options, "issue_type": None, "issue_types": [], "teams": [], "statuses": [],
    "defaults": {**options["defaults"], "issue_type": None, "team": None},
    "capabilities": {
        "can_create": False, "unassigned_allowed": False,
        **{
            field: {"mode": "unknown", "required": False, "diagnostic": None}
            for field in ("story_points", "team", "sprint")
        },
        "blockers": [{"code": "issue_type_required", "field": "issue_type", "message": "Select a supported issue type."}],
    },
}
validate_value(schema_for("JiraOptions"), no_type)
validate_value(schema_for("JiraOptions"), {**options, "assignees": []})
# Duplicate names may exist in upstream inventory, but name-based submission must
# fail server-side unless the selected name uniquely resolves. Never hide IDs or
# silently choose the first name match. Runtime resolution is outside this gate.
validate_value(schema_for("JiraOptions"), {
    **options, "statuses": [{"id": "a", "name": "Open"}, {"id": "b", "name": "Open"}],
})
examples += 3
for field, invalid in (("assignee_id", "jira-me"), ("sprint_id", "89396")):
    reject("JiraDefaults", {**options["defaults"], field: invalid})
    reject("UpstreamIssueDraft", {**draft, field: invalid})
pr_draft = {
    key: value for key, value in draft.items()
    if key != "upstream_issue_id"
}
pr_draft.update({"pull_request_id": "work-github:acme/widget:42", "generated_by": "ai", "warning": None})
validate_value(schema_for("TicketDraft"), pr_draft)
examples += 1
for field, invalid in (("assignee_id", "jira-me"), ("sprint_id", "89396"), ("suggested_status", "Review")):
    reject("TicketDraft", {**pr_draft, field: invalid})
for field in ("id", "name"):
    reject("StatusChoice", {**new_examples["StatusChoice"], field: ""})
reject("JiraAssignee", {**new_examples["JiraAssignee"], "email": 42})
story = documents["examples/options-story.json"]
if not (
    story["project_key"] == options["project_key"]
    and story["issue_type"] != options["issue_type"]
    and story["statuses"] != options["statuses"]
    and story["sprints"] == options["sprints"]
    and story["assignees"] == options["assignees"]
):
    raise ValueError("Type-scoped status fixture must differ without restricting sprint/assignee pools")
for path, value in documents.items():
    if path.startswith("examples/options-") and value["diagnostics"]:
        raise ValueError("Optional empty example inputs must not produce warning diagnostics")

for invalid in ("", None, 42):
    reject("UpstreamIssueDraftRequest", {"upstream_issue_id": invalid, "project_key": None})
    if invalid is not None:
        reject("ResolveUpstreamIssueRequest", {"source_id": invalid, "url": manual["url"]})
resolve_example = new_examples["ResolveUpstreamIssueRequest"]
for invalid in ("", "not a URL", "file:///etc/passwd", "ftp://github.com/acme/widget/issues/7", "x" * 4097):
    reject("ResolveUpstreamIssueRequest", {**resolve_example, "url": invalid})
for invalid in ("jira", "gitea", "unknown"):
    reject("UpstreamIssue", {**manual, "forge": invalid})
reject("UpstreamIssue", {**manual, "number": 0})
reject("UpstreamIssue", {**manual, "url": "javascript:alert(1)"})
reject("UpstreamIssue", {**manual, "pull_request": {"url": "https://forge.example/acme/widget/pulls/11"}})

healthy = documents["examples/sync-completed.json"]["sources"][0]
reject("SourceHealth", {**healthy, "status": "partial"})
reject("SourceHealth", {**healthy, "error": "Not a successful refresh"})
reject("SourceHealth", {**healthy, "status": "backoff"})
reject("SourceHealth", {**healthy, "last_success_at": "not-a-timestamp"})

progress = schemas["SourceProgress"]["examples"]
queued = next(p for p in progress if p["phase"] == "queued")
completed = next(p for p in progress if p["phase"] == "completed")
fetching = next(p for p in progress if p["phase"] == "fetching")
preparing = next(p for p in progress if p["phase"] == "preparing")
skipped = progress[-1]
reject("SourceProgress", {**queued, "completed_items": 1})
reject("SourceProgress", {**queued, "total_items": 25})
reject("SourceProgress", {**preparing, "completed_items": 25})
reject("SourceProgress", {**completed, "total_items": None})
reject("SourceProgress", {**completed, "completed_items": 24})
reject("SourceProgress", {**fetching, "total_items": 6})
reject("SourceProgress", {**fetching, "finished_at": "2026-11-09T12:00:14Z"})
reject("SourceProgress", {**completed, "elapsed_ms": -1})
reject("SourceProgress", {**skipped, "completed_items": 2})
reject("SourceProgress", {**completed, "phase": "invented"})

status = documents["examples/sync-completed.json"]
reject("SyncStatus", {**status, "progress": status["progress"][:-1]})
reject("SyncStatus", {**status, "progress": list(reversed(status["progress"]))})
reject("SyncStatus", {**status, "state": "syncing"})
reject("SyncStatus", {**status, "finished_at": None})
reject("SyncStatus", {**status, "started_at": None})
reject("SyncStatus", {**status, "progress": [queued, *status["progress"][1:]]})

# Ticket-first wire examples and cross-field rules, not an execution/concurrency test.
options_active = documents["examples/sync-options-active.json"]
options_failed = documents["examples/sync-options-failed.json"]
options_unavailable = documents["examples/sync-options-unavailable.json"]
options_discovering = documents["examples/sync-options-discovering.json"]
options_status = options_active["jira_options"]
options_progress = options_status["progress"]
complete_options = status["jira_options"]
complete_progress = complete_options["progress"]
startup = documents["examples/sync-startup.json"]
startup_options = startup["jira_options"]


def accept(name, value):
    global examples
    validate_value(schema_for(name), value)
    examples += 1


# These paired fixtures deliberately freeze ticket timing at independent commits.
# An options-only persisted-health commit advances the shared revision, not ticket
# success or job success. Counts are mock checks (positive/negative), not pool sizes.
if not (
    options_active["progress"] == options_failed["progress"]
    and options_active["sources"] == options_failed["sources"]
    and options_active["board_revision"] < options_failed["board_revision"]
    and options_active["last_success_at"] == options_failed["last_success_at"]
    and options_failed["jira_options"]["ready"]
    and not options_unavailable["jira_options"]["ready"]
):
    raise ValueError("Options failure fixtures must preserve committed tickets independently")
if options_progress["projects"] != [
    {"name": "IDM", "phase": "completed", "completed_items": 75, "total_items": 75},
    {"name": "RHEL", "phase": "fetching", "completed_items": 28, "total_items": 75},
]:
    raise ValueError("Eligibility fixture must demonstrate per-project checks, not ticket counts")

# Disabled Jira has no options object, no fake metadata/group sources. Restored
# verified choices may be ready while process-local progress is still startup idle.
forge_only = {
    **options_active,
    "sources": options_active["sources"][1:],
    "progress": options_active["progress"][1:],
    "jira_options": None,
}
accept("SyncStatus", forge_only)
accept("SyncStatus", {
    **startup, "board_revision": 25,
    "jira_options": {**complete_options, "progress": startup_options["progress"]},
})
# A first options attempt is unavailable until full commit even with fresh cards.
accept("SyncStatus", {
    **options_active,
    "jira_options": {
        **options_status, "ready": False,
        "health": {**options_status["health"], "status": "pending", "last_success_at": None},
    },
})
# Stale complete options remain usable throughout a later active retry.
accept("SyncStatus", {
    **options_active,
    "jira_options": {
        **options_status,
        "health": {**options_status["health"], "status": "error", "error": "Previous options discovery failed."},
    },
})
accept("SyncStatus", {
    **options_discovering,
    "jira_options": {
        **options_discovering["jira_options"],
        "progress": {**options_discovering["jira_options"]["progress"], "phase": "preparing"},
    },
})
# Metadata/groups may overlap remaining forge card reads; eligibility may not.
overlapping_discovery = copy.deepcopy(options_discovering)
overlapping_discovery["sources"][1]["last_success_at"] = "2026-11-09T11:00:30Z"
overlapping_discovery["progress"][1].update({"phase": "fetching", "finished_at": None})
accept("SyncStatus", overlapping_discovery)
accept("SyncStatus", {
    **options_active,
    "jira_options": {
        **options_status,
        "progress": {**options_progress, "phase": "publishing", "projects": complete_progress["projects"]},
    },
})
accept("JiraOptionsProgress", {
    **options_progress, "groups": [],
    "projects": [{"name": "IDM", "phase": "queued", "completed_items": 0, "total_items": 75}],
})
accept("JiraOptionsProgress", {
    **complete_progress, "groups": [],
    "projects": [{"name": "IDM", "phase": "completed", "completed_items": 0, "total_items": 0}],
})
accept("JiraOptionsProgress", {
    **complete_progress,
    "groups": [{"name": "empty-group", "phase": "completed", "completed_items": 0, "total_items": 0}],
})
# Skipped backoff resets progress without advancing persisted attempt/success.
options_backoff = {
    **complete_options,
    "health": {
        **complete_options["health"], "status": "backoff",
        "error": "Upstream rate limit; retry after the indicated time.",
        "retry_at": "2026-11-09T12:10:00Z",
    },
    "progress": {**startup_options["progress"], "phase": "backoff"},
}
accept("JiraOptionsStatus", options_backoff)
accept("JiraOptionsStatus", {
    **options_backoff, "ready": False,
    "health": {**options_backoff["health"], "last_success_at": None},
})
accept("JiraOptionsStatus", {
    **options_backoff,
    "progress": {
        **options_failed["jira_options"]["progress"], "phase": "backoff",
        "projects": [
            {**row, "phase": "backoff"} if row["phase"] == "failed" else row
            for row in options_failed["jira_options"]["progress"]["projects"]
        ],
    },
    "health": {**options_backoff["health"], "last_attempt_at": options_progress["started_at"]},
})
accept("JiraOptionsStatus", {
    **options_unavailable["jira_options"],
    "health": {**options_unavailable["jira_options"]["health"], "last_attempt_at": None},
    "progress": {
        **startup_options["progress"], "phase": "failed", "finished_at": "2026-11-09T12:00:20Z",
    },
})
# An options-only successful commit also signals a new board revision. The wire
# gate intentionally does not pretend to test SQLite atomicity or frontend races.
accept("SyncStatus", {
    **options_failed, "board_revision": options_failed["board_revision"] + 1,
    "last_success_at": options_failed["finished_at"],
    "jira_options": {
        **complete_options,
        "health": {
            **complete_options["health"], "last_attempt_at": options_progress["started_at"],
            "last_success_at": options_failed["finished_at"],
        },
        "progress": {
            **complete_progress, "started_at": options_progress["started_at"],
            "finished_at": options_failed["finished_at"], "elapsed_ms": 28000,
        },
    },
})

for invalid in (-1, 18446744073709551616, 1.5, "12", None, True):
    reject("SyncStatus", {**status, "board_revision": invalid})
    reject("Board", {**board, "revision": invalid})
accept("SyncStatus", {**status, "board_revision": 18446744073709551615})
accept("Board", {**board, "revision": 18446744073709551615})
reject("SyncStatus", {**status, "jira_options": None})
reject("SyncStatus", {**forge_only, "jira_options": options_status})
reject("SyncStatus", {**options_failed, "last_success_at": options_failed["finished_at"]})
reject("SyncStatus", {**status, "last_success_at": "2026-11-09T11:00:30Z"})
reject("JiraOptionsStatus", {**options_status, "ready": False})
reject("JiraOptionsStatus", {**startup_options, "ready": True})
reject("JiraOptionsStatus", {**complete_options, "ready": "true"})
for field, invalid in (("source_id", "jira-options"), ("kind", "github"), ("last_attempt_at", None)):
    reject("JiraOptionsStatus", {**options_status, "health": {**options_status["health"], field: invalid}})
reject("JiraOptionsStatus", {
    **options_status, "health": {**options_status["health"], "last_attempt_at": "2026-11-09T12:00:00Z"},
})
reject("JiraOptionsStatus", {
    **complete_options, "health": {**complete_options["health"], "last_success_at": "2026-11-09T11:00:30Z"},
})
reject("JiraOptionsStatus", {**options_failed["jira_options"], "health": options_status["health"]})
reject("JiraOptionsStatus", {**options_backoff, "health": complete_options["health"]})
for invalid in ("", " \t\n", None, 42):
    reject("ChoiceProgress", {**options_progress["projects"][1], "name": invalid})
for field, invalid in (("completed_items", -1), ("total_items", -1), ("total_items", 27), ("total_items", None), ("completed_items", True), ("phase", "invented")):
    reject("ChoiceProgress", {**options_progress["projects"][1], field: invalid})
reject("ChoiceProgress", {**options_progress["projects"][0], "completed_items": 74})
reject("ChoiceProgress", {"name": "IDM", "phase": "queued", "completed_items": 1, "total_items": 75})
reject("ChoiceProgress", {"name": "IDM", "phase": "failed", "completed_items": 28, "total_items": None})
reject("JiraOptionsProgress", {**options_progress, "groups": options_progress["groups"] * 2})
reject("JiraOptionsProgress", {**options_progress, "projects": options_progress["projects"] * 2})
reject("JiraOptionsProgress", {**complete_progress, "projects": []})
reject("JiraOptionsProgress", {**options_progress, "phase": "completed"})
reject("JiraOptionsProgress", {**options_progress, "phase": "publishing"})
reject("JiraOptionsProgress", {**options_progress, "started_at": None})
reject("JiraOptionsProgress", {**options_progress, "finished_at": "2026-11-09T12:00:30Z"})
reject("JiraOptionsProgress", {**options_progress, "elapsed_ms": -1})
reject("JiraOptionsProgress", {
    **options_failed["jira_options"]["progress"],
    "projects": [{"name": "RHEL", "phase": "failed", "completed_items": 28, "total_items": None}],
})
reject("JiraOptionsProgress", {**options_progress, "phase": "queued"})
reject("JiraOptionsProgress", {**options_progress, "phase": "failed", "finished_at": "2026-11-09T12:00:30Z"})
reject("JiraOptionsProgress", {**options_backoff["progress"], "elapsed_ms": 100})
reject("JiraOptionsProgress", {**options_backoff["progress"], "finished_at": "2026-11-09T12:00:30Z"})
reject("JiraOptionsProgress", {
    **options_progress, "groups": [{"name": "example-group", "phase": "discovering", "completed_items": 28, "total_items": 75}],
})
reject("JiraOptionsProgress", {
    **options_progress, "groups": [{"name": "example-group", "phase": "fetching", "completed_items": 0, "total_items": 75}],
})
# No eligibility while a group is incomplete, metadata is still discovering, or
# another source is fetching cards; no creation discovery ahead of Jira cards.
reject("JiraOptionsProgress", {
    **options_progress, "groups": options_discovering["jira_options"]["progress"]["groups"],
})
reject("JiraOptionsProgress", {
    **options_discovering["jira_options"]["progress"], "projects": options_progress["projects"],
})
for source_id in ("jira", "work-github"):
    invalid = copy.deepcopy(options_active)
    entry = next(entry for entry in invalid["progress"] if entry["source_id"] == source_id)
    entry.update({"phase": "fetching", "finished_at": None})
    reject("SyncStatus", invalid)
invalid = copy.deepcopy(options_discovering)
invalid["progress"][0].update({"phase": "fetching", "finished_at": None})
reject("SyncStatus", invalid)
# Stale choices and newer display sprints are allowed to differ. This is not a
# cross-response generation merge; options/draft/create validation uses its own cache.
early_context = documents["examples/context-ticket-first.json"]
if early_context["current_project"]["issue_types"] or not early_context["current_sprints"]:
    raise ValueError("Early real project/sprints fixture must not require creation metadata")
accept("Context", {**early_context, "current_sprints": []})

simulation = schemas["CreateIssueResult"]["examples"][0]
reject("CreateIssueResult", {**simulation, "issue": {
    "key": "FAKE-1", "url": "https://jira.example/browse/FAKE-1", "title": "Fake"
}})
# The schema examples above accept the existing malformed-success-body safeguard:
# created + null issue + explicit warning, without relaxing simulated's null-only rule.
accepted = next(
    example for example in schemas["CreateIssueResult"]["examples"]
    if example["outcome"] == "created" and example["issue"] is None
)
for invalid_warnings in ([], [""], [" \t\n"]):
    reject("CreateIssueResult", {**accepted, "warnings": invalid_warnings})
missing_warnings = {key: value for key, value in accepted.items() if key != "warnings"}
reject("CreateIssueResult", missing_warnings)
reject("RuntimeMode", {"read_only_test_mode": "false", "mode_revision": "mock"})
reject("RuntimeMode", {"read_only_test_mode": True})

for name, value in negative_cases:
    try:
        validate_value(schema_for(name), value)
    except (ValidationError, ValueError):
        continue
    raise ValueError(f"Invalid {name} unexpectedly accepted: {value}")

print(
    f"PASS: {len(documents)} JSON documents; OpenAPI {spec['info']['version']}; "
    f"{len(schemas)} schemas; {refs} local references; {examples} valid examples; "
    f"{len(negative_cases)} rejected invalid cases."
)
print("Not tested: server compatibility, independent SQLite publication/retention/quarantine, owned scheduling/concurrency/backoff/cancellation, upstream pagination/URL/SSRF enforcement, resolution-store lifecycle, Jira writes/direct-transition behavior, mode races/restarts or UI polling/accessibility.")

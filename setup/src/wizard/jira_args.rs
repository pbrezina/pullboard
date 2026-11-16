//! Targeted verification and explicit review of nonsecret Jira CLI settings.
use super::*;
use crate::cli::JiraArgs;

async fn verify<T, F: Future<Output = Result<T>>>(
    p: &mut impl Prompts,
    cancel: &CancellationToken,
    j: &JiraConfig,
    operation: &str,
    mut work: impl FnMut() -> F,
) -> Result<Option<T>> {
    loop {
        match cancellable(cancel, work()).await {
            Ok(value) => return Ok(Some(value)),
            Err(error) if error.is::<Cancelled>() => return Err(error),
            Err(error) => jira_error(p, operation, &error, j),
        }
        match p
            .select(
                "Supplied Jira setting failed:",
                &choices(&[
                    "Retry",
                    "Correct this setting interactively",
                    "Cancel setup",
                ]),
                0,
            )
            .await?
        {
            0 => {}
            1 => return Ok(None),
            _ => return Err(Cancelled.into()),
        }
    }
}

pub(super) async fn projects(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &mut JiraConfig,
    original: &Config,
    args: &mut JiraArgs,
) -> Result<bool> {
    let Some(keys) = &args.projects else {
        return Ok(false);
    };
    let verified = verify(p, cancel, j, "Verify --jira-projects", || async {
        for key in &keys.0 {
            let found = d.project(j, key).await?;
            ensure!(
                found.len() == 1 && found[0].id == *key,
                "project lookup did not verify a unique exact project key"
            );
        }
        Ok(())
    })
    .await?;
    if verified.is_some() {
        let cleanup = project_cleanup(original, &keys.0);
        for line in &cleanup {
            p.say(&clean(line));
        }
        if cleanup.is_empty()
            || p.confirm(
                "Confirm project removal and this dependent-setting cleanup?",
                false,
            )
            .await?
        {
            j.projects.clone_from(&keys.0);
            j.project_overrides
                .retain(|key, _| j.projects.contains(key));
            return Ok(true);
        }
    }
    // Never reapply a rejected argument when the user corrects it or retries Jira.
    args.projects = None;
    Ok(false)
}

pub(super) async fn board(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &mut JiraConfig,
    args: &mut JiraArgs,
) -> Result<bool> {
    if args.board_ids.is_empty() {
        return Ok(false);
    }
    if verify(p, cancel, j, "Verify --jira-board-id", || async {
        for id in &args.board_ids {
            let board = d.board(j, &id.to_string()).await?;
            ensure!(
                board.id == id.to_string(),
                "board lookup returned a different ID"
            );
        }
        Ok(())
    })
    .await?
    .is_some()
    {
        j.board_ids.clone_from(&args.board_ids);
        return Ok(true);
    }
    args.board_ids.clear();
    Ok(false)
}

pub(super) async fn groups(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &mut JiraConfig,
    args: &mut JiraArgs,
) -> Result<bool> {
    let Some(names) = &args.groups else {
        return Ok(false);
    };
    if verify(
        p,
        cancel,
        j,
        "Verify --jira-groups and membership-read access",
        || async {
            for name in &names.0 {
                let group = d.group(j, name).await?;
                ensure!(group.id == *name, "group lookup returned a different name");
            }
            Ok(())
        },
    )
    .await?
    .is_some()
    {
        j.assignee_groups.clone_from(&names.0);
        return Ok(true);
    }
    args.groups = None;
    Ok(false)
}

pub(super) async fn field(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &mut JiraConfig,
    fields: &mut Vec<Choice>,
    args: &mut JiraArgs,
) -> Result<bool> {
    let Some(name) = &args.team_field else {
        return Ok(false);
    };
    let jira: &JiraConfig = j;
    let mut initial = Some(fields.clone());
    if let Some(verified) = verify(p, cancel, jira, "Verify --jira-team-field", || {
        let initial = initial.take();
        async move {
            let fields = match initial {
                Some(fields) => fields,
                None => team_fields(&d.settings(jira).await?),
            };
            ensure!(
                fields.iter().filter(|field| field.id == *name).count() == 1,
                "no unique supported semantic team field has the supplied name"
            );
            Ok(fields)
        }
    })
    .await?
    {
        *fields = verified;
        j.team_field_name.clone_from(name);
        return Ok(true);
    }
    args.team_field = None;
    Ok(false)
}

pub(super) async fn team(
    p: &mut impl Prompts,
    d: &impl Discovery,
    cancel: &CancellationToken,
    j: &mut JiraConfig,
    metadata: &mut Metadata,
    args: &mut JiraArgs,
) -> Result<bool> {
    let Some(name) = &args.team else {
        return Ok(false);
    };
    let jira: &JiraConfig = j;
    let mut initial = Some(metadata.clone());
    if let Some(verified) = verify(p, cancel, jira, "Verify --jira-team", || {
        let initial = initial.take();
        async move {
            let metadata = match initial {
                Some(metadata) => metadata,
                None => d.metadata(jira).await?,
            };
            ensure!(
                metadata_teams(jira, &metadata)
                    .iter()
                    .any(|team| team.id == *name),
                "supplied team is not a verified selected-project/type create option"
            );
            Ok(metadata)
        }
    })
    .await?
    {
        *metadata = verified;
        j.team.clone_from(name);
        return Ok(true);
    }
    args.team = None;
    Ok(false)
}

pub(super) fn changes(args: &JiraArgs, j: &JiraConfig) -> Vec<String> {
    let mut lines = Vec::new();
    for (label, before, after) in [
        ("Jira URL", j.base_url.clone(), args.url.clone()),
        (
            "Projects (primary first)",
            j.projects.join(", "),
            args.projects.as_ref().map(|v| v.0.join(", ")),
        ),
        (
            "Boards (My board first)",
            boards(&j.board_ids),
            (!args.board_ids.is_empty()).then(|| boards(&args.board_ids)),
        ),
        (
            "Assignee groups",
            j.assignee_groups.join(", "),
            args.groups.as_ref().map(|v| v.0.join(", ")),
        ),
        (
            "Team field",
            j.team_field_name.clone(),
            args.team_field.clone(),
        ),
        ("Team", j.team.clone(), args.team.clone()),
        (
            "Sprint regexes (My pattern first)",
            patterns(&j.sprint_name_patterns),
            (!args.sprint_name_patterns.is_empty()).then(|| patterns(&args.sprint_name_patterns)),
        ),
    ] {
        if let Some(after) = after {
            lines.push(clean(&format!("{label}: {before} → {after}")));
        }
    }
    lines
}

pub(super) fn summary(j: &JiraConfig) -> Vec<String> {
    vec![
        format!(
            "Jira: {} · projects {} (primary first) · boards {} (My board first) · team {}",
            j.base_url,
            j.projects.join(", "),
            boards(&j.board_ids),
            j.team
        ),
        format!(
            "Assignee groups: {} · team field: {}",
            j.assignee_groups.join(", "),
            j.team_field_name
        ),
        format!(
            "Regular expressions to identify your sprints (My pattern first): {}",
            patterns(&j.sprint_name_patterns)
        ),
    ]
}

fn boards(ids: &[u64]) -> String {
    ids.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn patterns(values: &[SprintPattern]) -> String {
    values
        .iter()
        .map(SprintPattern::as_str)
        .collect::<Vec<_>>()
        .join(" | ")
}

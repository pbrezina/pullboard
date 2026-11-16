use pullboard_core::config::Config;

const YAML: &str = "jira:\n  base_url: https://jira.example\n  email: human@example.invalid\n  token: {value: synthetic}\n  projects: [DEV]\n  board_ids: [14585, 11235]\n  team: Team\n  sprint_name_patterns: ['SSSD-IDENTITY', 'SSSD-CORE']\n";

#[test]
fn ordered_lists_round_trip_without_sorting_and_have_independent_matching_roles() {
    let config = Config::from_yaml(YAML).unwrap();
    let config = Config::from_yaml(&config.to_yaml().unwrap()).unwrap();
    let j = config.jira.unwrap();
    assert_eq!(j.board_ids, [14585, 11235]);
    assert_eq!(
        j.sprint_name_patterns
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>(),
        ["SSSD-IDENTITY", "SSSD-CORE"]
    );
    assert!(j.current_sprint(14585, "active", "2026-SSSD-IDENTITY-S1"));
    assert!(!j.current_sprint(11235, "active", "2026-SSSD-IDENTITY-S1"));
    assert!(!j.current_sprint(14585, "future", "2026-SSSD-IDENTITY-S1"));
    assert!(!j.current_sprint(14585, "active", "2026-SSSD-CORE-S1"));
    assert!(j.allows_sprint("2026-SSSD-CORE-S1"));
    assert!(!j.allows_sprint("2026-UAH-S1"));
}

#[test]
fn invalid_missing_empty_duplicate_overlong_and_singular_lists_are_rejected() {
    for replacement in ["[]", "[0]", "[-1]", "[7, 7]", "['7']", "null", "7"] {
        assert!(Config::from_yaml(&YAML.replace("[14585, 11235]", replacement)).is_err());
    }
    for replacement in [
        "[]",
        "['']",
        "[' ']",
        "['[']",
        "['(?=a)']",
        "[Team, Team]",
        "[1]",
        "null",
        "Team",
    ] {
        assert!(
            Config::from_yaml(&YAML.replace("['SSSD-IDENTITY', 'SSSD-CORE']", replacement))
                .is_err()
        );
    }
    for field in [
        "  board_ids: [14585, 11235]\n",
        "  sprint_name_patterns: ['SSSD-IDENTITY', 'SSSD-CORE']\n",
    ] {
        assert!(Config::from_yaml(&YAML.replace(field, "")).is_err());
    }
    for (plural, singular) in [
        ("board_ids", "board_id"),
        ("sprint_name_patterns", "sprint_name_pattern"),
    ] {
        assert!(Config::from_yaml(&YAML.replace(plural, singular)).is_err());
    }
    let boards = format!(
        "[{}]",
        (1..=101)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let patterns = format!(
        "[{}]",
        (1..=101)
            .map(|n| format!("'Team{n}'"))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(Config::from_yaml(&YAML.replace("[14585, 11235]", &boards)).is_err());
    assert!(Config::from_yaml(&YAML.replace("['SSSD-IDENTITY', 'SSSD-CORE']", &patterns)).is_err());
    for pattern in ["a".repeat(4097), "(?:a{1000}){1000}".into()] {
        assert!(Config::from_yaml(&YAML.replace("SSSD-IDENTITY", &pattern)).is_err());
    }
}

#[test]
fn inclusive_hundred_entry_bounds_preserve_order_and_regex_search() {
    let boards = format!(
        "[{}]",
        (1..=100)
            .rev()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let patterns = format!(
        "[{}]",
        (1..=100)
            .rev()
            .map(|n| format!("'Team{n}'"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let c = Config::from_yaml(
        &YAML
            .replace("[14585, 11235]", &boards)
            .replace("['SSSD-IDENTITY', 'SSSD-CORE']", &patterns),
    )
    .unwrap();
    let j = c.jira.unwrap();
    assert_eq!(j.board_ids.len(), 100);
    assert_eq!(j.board_ids[0], 100);
    assert_eq!(j.sprint_name_patterns[0].as_str(), "Team100");
}

#[test]
fn programmatic_mutations_are_validated_without_recompiling_or_deduplicating() {
    let mut c = Config::from_yaml(YAML).unwrap();
    c.jira.as_mut().unwrap().board_ids.reverse();
    c.validate().unwrap();
    c.jira.as_mut().unwrap().board_ids.push(11235);
    assert!(c.validate().is_err());
    let mut c = Config::from_yaml(YAML).unwrap();
    let j = c.jira.as_mut().unwrap();
    j.sprint_name_patterns
        .push(j.sprint_name_patterns[0].clone());
    assert!(c.validate().is_err());
}

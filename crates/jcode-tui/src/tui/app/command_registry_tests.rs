use super::*;

/// Every slash command must be registered exactly once. Duplicate entries
/// mean two different handlers claim the same name, so which one runs
/// depends on dispatch order rather than on the registry the palette and
/// `/help` show the user.
#[test]
fn registered_commands_have_no_duplicate_names() {
    let mut seen = std::collections::HashSet::new();
    let duplicates: Vec<&str> = REGISTERED_COMMANDS
        .iter()
        .filter(|command| !seen.insert(command.name))
        .map(|command| command.name)
        .collect();
    assert!(
        duplicates.is_empty(),
        "duplicate slash command registrations: {:?}",
        duplicates
    );
}

/// Aliases users can actually type must be discoverable through the
/// registry, otherwise autocomplete silently omits working commands.
#[test]
fn known_aliases_are_registered() {
    let names: std::collections::HashSet<&str> =
        REGISTERED_COMMANDS.iter().map(|c| c.name).collect();
    for alias in [
        "/keybindings",
        "/commit-and-push",
        "/resume-all",
        "/hotkeys",
        "/keys",
    ] {
        assert!(names.contains(alias), "{alias} is not registered");
    }
}

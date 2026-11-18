use super::*;

#[test]
fn parses_mobile_as_a_move_to_managed_cloud() {
    for input in ["/mobile", "/mobile ", " /mobile\t"] {
        assert_eq!(
            parse_cloud_command(input),
            Some(CloudCommand::Move {
                host: Some("jcode-cloud".into()),
            }),
            "{input:?}"
        );
    }
}

#[test]
fn parses_mobile_cancel() {
    for input in ["/mobile cancel", "/mobile\tcancel "] {
        assert_eq!(
            parse_cloud_command(input),
            Some(CloudCommand::Cancel),
            "{input:?}"
        );
    }
}

#[test]
fn mobile_rejects_unexpected_arguments() {
    for input in [
        "/mobile box",
        "/mobile --host box",
        "/mobile --session-only",
        "/mobile --include-session other",
        "/mobile --dry-run",
        "/mobile status",
        "/mobile cancel extra",
        "/mobile cancel cancel",
    ] {
        assert_eq!(
            parse_cloud_command(input),
            Some(CloudCommand::MoveUsage),
            "{input}"
        );
    }
}

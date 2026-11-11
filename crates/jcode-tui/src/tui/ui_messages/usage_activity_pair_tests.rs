use super::*;

#[test]
fn incomplete_activity_pairs_keep_original_details_and_tracking() {
    for period in ["Today", "Lifetime"] {
        let content =
            format!("*ok\tReady\n: {period}\tNo recorded usage\n: Tracking\tcustom caveat");
        let card = ProviderCard::parse(&content).expect("valid card");
        let lines = card.body_lines(58, false);
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(period), "{text}");
        assert!(text.contains("No recorded usage"), "{text}");
        assert!(text.contains("Tracking"), "{text}");
        assert!(text.contains("custom caveat"), "{text}");
        assert!(!text.contains("Jcode usage"), "{text}");
        for width in [1, 12, 24, 64] {
            assert!(
                card.render("OpenAI", width)
                    .iter()
                    .all(|line| line.width() <= width)
            );
        }
    }
}

#[test]
fn complete_activity_pair_renders_summary_once_in_any_row_order() {
    for rows in [
        ": Today\tNo recorded usage\n: Lifetime\t100 in · 20 out · cost unknown\n: Tracking\told caveat",
        ": Tracking\told caveat\n: Lifetime\t100 in · 20 out · cost unknown\n: Today\tNo recorded usage",
    ] {
        let content = format!("*ok\tReady\n{rows}");
        let card = ProviderCard::parse(&content).expect("valid card");
        let text = card
            .body_lines(58, false)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text.matches("Jcode usage").count(), 1, "{text}");
        assert!(text.contains("Today"), "{text}");
        assert!(text.contains("Lifetime"), "{text}");
        assert!(!text.contains("old caveat"), "{text}");
    }
}

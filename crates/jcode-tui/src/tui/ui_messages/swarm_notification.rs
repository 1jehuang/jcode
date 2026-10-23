use super::{Color, dim_color, rgb};

pub(super) struct CompactSwarmNotification<'a> {
    pub(super) sender: &'a str,
    pub(super) marker: String,
    pub(super) marker_before_icon: bool,
    pub(super) text_color: Color,
    pub(super) file_activity: bool,
}

pub(super) fn compact_swarm_notification(title: &str) -> Option<CompactSwarmNotification<'_>> {
    let (sender, marker, marker_before_icon, text_color, file_activity) =
        if let Some(sender) = title.strip_prefix("DM from ") {
            let sender = sender
                .split_once(" (swarm '")
                .filter(|_| sender.ends_with("')"))
                .map_or(sender, |(name, _)| name);
            (sender, String::new(), false, rgb(130, 130, 140), false)
        } else if let Some(sender) = title.strip_prefix("Task · ") {
            (sender, String::new(), false, rgb(220, 236, 255), false)
        } else if let Some((channel, sender)) = title
            .strip_prefix('#')
            .and_then(|rest| rest.rsplit_once(" · "))
        {
            (
                sender,
                format!("#{channel} · "),
                false,
                rgb(214, 247, 244),
                false,
            )
        } else if let Some(sender) = title.strip_prefix("Broadcast · ") {
            (sender, "📣 ".to_string(), false, rgb(255, 240, 214), false)
        } else if let Some(sender) = title.strip_prefix("Shared context · ") {
            (sender, "🧠 ".to_string(), false, rgb(221, 247, 232), false)
        } else if let Some(sender) = title.strip_prefix("File activity · ") {
            (sender, "✎ ".to_string(), false, dim_color(), true)
        } else if title == "File activity" {
            ("", "✎ ".to_string(), false, dim_color(), true)
        } else if let Some(sender) = title.strip_prefix("File conflict · ") {
            (sender, "⚠ ".to_string(), true, rgb(255, 190, 150), true)
        } else {
            let sender = title.strip_prefix("Swarm · ")?;
            (sender, String::new(), false, rgb(225, 225, 235), false)
        };
    let sender = sender.trim();
    (!sender.is_empty() || title == "File activity").then_some(CompactSwarmNotification {
        sender,
        marker,
        marker_before_icon,
        text_color,
        file_activity,
    })
}

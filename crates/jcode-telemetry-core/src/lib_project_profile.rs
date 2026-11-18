use super::*;

pub(super) fn update_active_days(id: &str) -> (u32, u32) {
    let Some(path) = active_days_path(id) else {
        return (0, 0);
    };
    let today = Utc::now().date_naive();
    let mut days = std::fs::read_to_string(&path)
        .ok()
        .into_iter()
        .flat_map(|text| {
            text.lines()
                .map(str::trim)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter_map(|line| NaiveDate::parse_from_str(&line, "%Y-%m-%d").ok())
        .collect::<Vec<_>>();
    days.push(today);
    days.sort_unstable();
    days.dedup();
    let rendered = days
        .iter()
        .map(NaiveDate::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    write_private_file(&path, &rendered);
    let days_7 = days
        .iter()
        .filter(|day| (today.signed_duration_since(**day).num_days()) < 7)
        .count()
        .min(u32::MAX as usize) as u32;
    let days_30 = days
        .iter()
        .filter(|day| (today.signed_duration_since(**day).num_days()) < 30)
        .count()
        .min(u32::MAX as usize) as u32;
    (days_7, days_30)
}

pub(super) fn detect_project_profile() -> ProjectProfile {
    fn keep_project_entry(entry: &walkdir::DirEntry) -> bool {
        if !entry.file_type().is_dir() {
            return true;
        }
        let name = entry.file_name().to_str().unwrap_or_default();
        !matches!(
            name,
            ".git" | "target" | "node_modules" | "dist" | "build" | ".next"
        )
    }

    let cwd = std::env::current_dir().ok();
    let mut profile = ProjectProfile::default();
    let Some(root) = cwd.as_deref() else {
        return profile;
    };
    profile.repo_present = root.join(".git").exists() || is_jcode_repo_dir(root);
    let mut scanned_files = 0usize;
    for entry in walkdir::WalkDir::new(root)
        .max_depth(3)
        .into_iter()
        .filter_entry(keep_project_entry)
        .filter_map(Result::ok)
    {
        if scanned_files >= 400 {
            break;
        }
        if entry.file_type().is_dir() {
            continue;
        }
        scanned_files += 1;
        profile.note_extension(
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default(),
        );
    }
    profile
}

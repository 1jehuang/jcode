use chrono::{DateTime, Utc};

use super::{
    OvernightManifest, OvernightPreflight, OvernightRunStatus, format_minutes, preflight_summary,
};

pub(crate) fn overnight_phase(manifest: &OvernightManifest, now: DateTime<Utc>) -> &'static str {
    match manifest.status {
        OvernightRunStatus::Completed => "completed",
        OvernightRunStatus::Failed => "failed",
        OvernightRunStatus::CancelRequested => "cancelling",
        OvernightRunStatus::Running => {
            if now < manifest.handoff_ready_at {
                "running"
            } else if now < manifest.target_wake_at {
                "wind-down"
            } else if manifest.morning_report_posted_at.is_none() {
                "morning report"
            } else if now < manifest.post_wake_grace_until {
                "post-wake"
            } else {
                "finalizing"
            }
        }
    }
}

pub(crate) fn time_relation_to_target(manifest: &OvernightManifest, now: DateTime<Utc>) -> String {
    let minutes = manifest
        .target_wake_at
        .signed_duration_since(now)
        .num_minutes();
    if minutes >= 0 {
        format!("target in {}", format_minutes(minutes as u32))
    } else {
        format!("target passed {} ago", format_minutes((-minutes) as u32))
    }
}

pub(crate) fn relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let minutes = now.signed_duration_since(then).num_minutes();
    if minutes >= 0 {
        format!("{} ago", format_minutes(minutes as u32))
    } else {
        format!("in {}", format_minutes((-minutes) as u32))
    }
}

pub(crate) fn next_prompt_label(manifest: &OvernightManifest, now: DateTime<Utc>) -> String {
    if !matches!(manifest.status, OvernightRunStatus::Running) {
        return "none".to_string();
    }
    if now < manifest.handoff_ready_at {
        return format!(
            "handoff mode in {} or after current turn",
            format_minutes(
                manifest
                    .handoff_ready_at
                    .signed_duration_since(now)
                    .num_minutes()
                    .max(0) as u32
            )
        );
    }
    if now < manifest.target_wake_at {
        return format!(
            "morning report in {} or after current turn",
            format_minutes(
                manifest
                    .target_wake_at
                    .signed_duration_since(now)
                    .num_minutes()
                    .max(0) as u32
            )
        );
    }
    if manifest.morning_report_posted_at.is_none() {
        return "morning report after current turn".to_string();
    }
    if now < manifest.post_wake_grace_until {
        return format!(
            "final wrap by {} or after current turn",
            manifest.post_wake_grace_until.format("%H:%M UTC")
        );
    }
    "final wrap after current turn".to_string()
}

pub fn build_coordinator_prompt(
    manifest: &OvernightManifest,
    preflight: &OvernightPreflight,
) -> String {
    let mission = manifest
        .mission
        .as_deref()
        .unwrap_or("Continue the current session's highest-value work, prioritizing verified, low-risk progress.");
    format!(
        r#"You are the Overnight Coordinator for Jcode run `{run_id}`.

The user expects to be away until approximately `{target_wake_at}`. This is a target wake/report time, not a hard stop. By that time, the run must be handoff-ready and the review page must explain what happened. You may continue past the target only to finish a bounded, safe, verifiable chunk. The default soft post-wake grace window ends at `{post_wake_grace_until}`.

Mission:
{mission}

Operating contract:
- Optimize for verified, low-risk progress.
- Prefer GH bug issues with objective reproduction, failing tests, static-analysis findings, regression tests, bounded code-quality fixes, and clear crash/panic/wrong-output bugs.
- Avoid taste-based work, vague product decisions, broad rewrites, risky migrations, payments, sending email, pushing to remotes, deleting data, or other external side effects unless explicitly allowed by the user.
- If a bug is found, reproduce/prove it before fixing it.
- Only fix issues that are important, bounded, and verifiable. Otherwise draft a high-quality issue in `{issue_drafts}`.
- You own the run. Spawn swarm/helper agents only if the expected value exceeds usage/resource cost. Default to one coordinator plus at most one helper. Read-only scouts/verifiers are preferred over multiple editors.
- Be aware of RAM/load/battery, especially around compiles, browser automation, indexing, and full test suites. Do not run multiple heavy activities at once unless resources are clearly healthy.
- Do not wait for the user. If you need user judgment/credentials/taste, record it and switch to another useful task.
- Continue finding useful verified work until the target wake/report time unless usage/resources make that unreasonable.

Review/log requirements:
- Keep `{review_notes}` updated as you work.
- For each meaningful task, maintain one structured JSON task card in `{task_cards}` using the schema in `{task_card_schema}`. These cards drive the live TUI progress card and the generated review page.
- Each task card must include clear Before/After, evidence, validation, files changed, risk, status, and outcome. Keep the current task marked `active`, completed verified work marked `completed`, user/taste/credential stalls marked `blocked`, and considered-but-not-pursued work marked `deferred` or `skipped`.
- Put reproduction/test/command outputs in `{validation}` when useful.
- The generated review page is `{review_html}` and will be regenerated from logs plus your review notes.

Preflight summary:
{preflight_summary}

Initial steps:
1. Inspect current repo/session state and git status.
2. Build a ranked queue of verifiable candidate tasks.
3. Pick the highest-confidence bounded task.
4. Prove/reproduce before fixing.
5. Validate and update review notes.
6. If done early, repeat discovery and continue.
"#,
        run_id = manifest.run_id,
        target_wake_at = manifest.target_wake_at.to_rfc3339(),
        post_wake_grace_until = manifest.post_wake_grace_until.to_rfc3339(),
        mission = mission,
        issue_drafts = manifest.issue_drafts_dir.display(),
        review_notes = manifest.review_notes_path.display(),
        task_cards = manifest.task_cards_dir.display(),
        task_card_schema = manifest
            .task_cards_dir
            .join("task-card-schema.md")
            .display(),
        validation = manifest.validation_dir.display(),
        review_html = manifest.review_path.display(),
        preflight_summary = preflight_summary(preflight),
    )
}

pub fn build_visible_current_session_prompt(manifest: &OvernightManifest) -> String {
    let mission = manifest
        .mission
        .as_deref()
        .unwrap_or("Continue the current session's highest-value work, prioritizing verified, low-risk progress.");
    format!(
        r#"You are now the visible Overnight Coordinator for Jcode run `{run_id}`.

The user expects this current session to become the overnight session. Keep all work visible here: your normal tool calls, any spawned/swarm helper agents, their reports, and validation should be observable from this session like a normal interactive run.

Important: because this is the visible current-session mode, there is no separate hidden supervisor loop running additional turns for you. You must self-manage the overnight lifecycle from this visible turn: check the target wake time yourself, post a morning report when it is reached, avoid continuing past the grace window except for a bounded safe wrap-up, and check the manifest for cancellation before starting each major new task.

Target wake/report time: `{target_wake_at}`
Soft post-wake grace window ends: `{post_wake_grace_until}`

Mission:
{mission}

Operating contract:
- Do not wait for the user. If you need user judgment/credentials/taste, record it and switch to another useful task.
- Optimize for verified, low-risk progress. Prefer objective bugs, repros, regression tests, bounded quality fixes, and clear validation.
- Avoid broad rewrites, taste-based decisions, risky migrations, payments, sending email, pushing to remotes, deleting data, or external side effects unless explicitly allowed.
- Spawn helper/swarm agents only when valuable, and keep their work headed/visible from this session. Prefer read-only scouts/verifiers over many editors.
- Watch RAM/load/battery and avoid concurrent heavy builds or tests unless resources are clearly healthy.

Review/log requirements:
- Keep `{review_notes}` updated as you work.
- For each meaningful task, maintain one task-card JSON in `{task_cards}` using `{task_card_schema}`.
- Task cards should include Before/After, evidence, validation, files changed, risk, status, and outcome.
- Put useful command outputs in `{validation}`.
- The generated review page is `{review_html}`.
- Manifest path: `{manifest_path}`. If cancellation is requested or the run completes, update the manifest/status consistently when safe.

Initial steps:
1. Inspect current repo/session state, including git status and current todos.
2. Build a ranked queue of verifiable candidate tasks.
3. Pick the highest-confidence bounded task.
4. Prove/reproduce before fixing.
5. Validate, update review notes/task cards, and continue with the next bounded task until the target wake/report time.
"#,
        run_id = manifest.run_id,
        target_wake_at = manifest.target_wake_at.to_rfc3339(),
        post_wake_grace_until = manifest.post_wake_grace_until.to_rfc3339(),
        mission = mission,
        review_notes = manifest.review_notes_path.display(),
        task_cards = manifest.task_cards_dir.display(),
        task_card_schema = manifest
            .task_cards_dir
            .join("task-card-schema.md")
            .display(),
        validation = manifest.validation_dir.display(),
        review_html = manifest.review_path.display(),
        manifest_path = manifest.run_dir.join("manifest.json").display(),
    )
}

pub fn build_continuation_prompt(manifest: &OvernightManifest) -> String {
    let remaining = manifest
        .target_wake_at
        .signed_duration_since(Utc::now())
        .num_minutes()
        .max(0) as u32;
    format!(
        "Overnight continuation: there is about {} remaining until the target wake/report time. If your current task is complete, run another discovery/scoring pass and choose another high-confidence, verifiable task. If you are stuck, record why in `{}` and the relevant task-card JSON, then switch to a smaller bounded task. Update review notes and task cards before continuing.",
        format_minutes(remaining),
        manifest.review_notes_path.display()
    )
}

pub fn build_handoff_ready_prompt(manifest: &OvernightManifest) -> String {
    format!(
        "Handoff-ready reminder: target wake/report time is in about 30 minutes. Do not abandon useful work, but make the run easy to understand. Update `{}` and task-card JSON with current task, completed work, validation state, files changed, risks, skipped work, and next steps. Avoid starting large/risky new changes unless they are isolated and clearly verifiable.",
        manifest.review_notes_path.display()
    )
}

pub fn build_morning_report_prompt(manifest: &OvernightManifest) -> String {
    format!(
        "Target wake/report time reached. Post a morning report now, even if work is still ongoing. Update `{}` plus task-card JSON and make sure `{}` is useful. Include completed work, current task, before/after evidence, files changed, validation, risks, usage/resource notes if relevant, and whether you plan to continue. You may continue only if the next chunk is bounded, safe, and verifiable.",
        manifest.review_notes_path.display(),
        manifest.review_path.display()
    )
}

pub fn build_post_wake_continuation_prompt(manifest: &OvernightManifest) -> String {
    format!(
        "Post-wake continuation: the target wake/report time has passed and the morning report should already be available. You may continue only with bounded, safe, verifiable work that is already in progress or clearly high-value. Do not start broad/risky new changes. Keep `{}` and task-card JSON current so the user can safely inspect or interrupt at any time. Soft grace window ends at `{}`.",
        manifest.review_notes_path.display(),
        manifest.post_wake_grace_until.to_rfc3339()
    )
}

pub fn build_final_wrapup_prompt(manifest: &OvernightManifest) -> String {
    format!(
        "Final overnight wrap-up: the post-wake grace window has expired. Stop starting new work. Finish only immediate cleanup, update `{}`, task-card JSON, and `{}` with final before/after evidence, validation status, dirty repo state, remaining risks, and next steps, then stop.",
        manifest.review_notes_path.display(),
        manifest.review_path.display()
    )
}

pub fn prompt_event_summary(prompt: &str) -> String {
    if prompt.starts_with("You are the Overnight Coordinator") {
        "Sending initial overnight coordinator mission".to_string()
    } else if prompt.starts_with("Handoff-ready") {
        "Sending handoff-ready poke".to_string()
    } else if prompt.starts_with("Target wake") {
        "Sending morning report poke".to_string()
    } else if prompt.starts_with("Post-wake continuation") {
        "Sending post-wake continuation poke".to_string()
    } else if prompt.starts_with("Final overnight wrap-up") {
        "Sending final wrap-up poke".to_string()
    } else {
        "Sending continuation poke".to_string()
    }
}

use super::*;
use anyhow::{Context, ensure};
use chrono::DateTime;
use std::collections::HashSet;

const API: &str = "https://api.github.com/repos/1jehuang/jcode";

fn sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Deserialize)]
struct Release {
    draft: bool,
    prerelease: bool,
    published_at: Option<DateTime<Utc>>,
    tag_name: String,
}

impl Release {
    fn reachable<'a>(&'a self, ancestors: &HashSet<&str>) -> Option<(DateTime<Utc>, &'a str)> {
        let id = self.tag_name.strip_prefix("selfdev-linux-")?;
        (self.prerelease && !self.draft && sha(id) && ancestors.contains(id))
            .then_some((self.published_at?, id))
    }
}

async fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = tokio::process::Command::new("git")
        .arg("--no-replace-objects")
        .args(args)
        .current_dir(repo)
        .env("GIT_NO_LAZY_FETCH", "1")
        .kill_on_drop(true)
        .output()
        .await?;
    ensure!(out.status.success(), "Git history unavailable");
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
}

async fn get<T: serde::de::DeserializeOwned>(client: &reqwest::Client, path: &str) -> Result<T> {
    let response = client
        .get(format!("{API}/{path}"))
        .send()
        .await
        .context("canonical GitHub API unavailable")?;
    ensure!(
        response.status().is_success(),
        "canonical GitHub API returned {}",
        response.status()
    );
    response
        .json()
        .await
        .context("invalid canonical GitHub response")
}

fn should_stop_release_pagination(
    rows: &[Release],
    latest: Option<&(DateTime<Utc>, String)>,
) -> bool {
    if rows.len() < 100 {
        return true;
    }
    let Some((latest_time, _)) = latest else {
        return false;
    };
    rows.iter()
        .rev()
        .find_map(|r| r.published_at)
        .is_some_and(|oldest| oldest < *latest_time)
}

async fn public_history(repo: &Path, text: &mut String) -> Result<()> {
    let head = git(repo, &["rev-parse", "HEAD"]).await?;
    ensure!(sha(&head), "invalid HEAD");
    let dirty = !git(repo, &["status", "--porcelain"]).await?.is_empty();
    text.push_str(&format!(
        "\nHEAD: {head}{}\n",
        if dirty {
            " (dirty, uncommitted changes excluded)"
        } else {
            ""
        }
    ));
    ensure!(
        git(repo, &["rev-parse", "--is-shallow-repository"]).await? == "false",
        "shallow checkout, release ancestry incomplete"
    );
    let history = git(repo, &["rev-list", &head]).await?;
    let ancestors = history.lines().collect::<HashSet<_>>();
    let client = reqwest::Client::builder()
        .user_agent("jcode-selfdev-history")
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut latest = None;
    for page in 1.. {
        let rows: Vec<Release> =
            get(&client, &format!("releases?per_page=100&page={page}")).await?;
        for (time, id) in rows.iter().filter_map(|r| r.reachable(&ancestors)) {
            if latest.as_ref().is_none_or(|(t, _)| time > *t) {
                latest = Some((time, id.to_owned()));
            }
        }
        if should_stop_release_pagination(&rows, latest.as_ref()) {
            break;
        }
    }
    let range = if let Some((time, id)) = latest {
        text.push_str(&format!(
            "Latest reachable published Linux selfdev prerelease: {time} {id}\n"
        ));
        format!("{id}..{head}")
    } else {
        text.push_str("No reachable published Linux selfdev prerelease. Showing public history through HEAD.\n");
        head.clone()
    };
    let range_ids = git(repo, &["rev-list", &range]).await?;
    if range_ids.is_empty() {
        text.push_str("No commits since publication.\n");
        return Ok(());
    }
    // Never send local SHAs, subjects, paths, or request context to GitHub. Only
    // print subjects whose objects are reachable from the live canonical branch.
    #[derive(Deserialize)]
    struct Tip {
        sha: String,
    }
    let tip: Tip = get(&client, "commits/master").await?;
    ensure!(sha(&tip.sha), "invalid public tip");
    let public = git(repo, &["rev-list", &tip.sha])
        .await
        .context("canonical master not available locally, fetch it to inspect public commits")?;
    let public: HashSet<_> = public.lines().collect();
    let subjects = git(
        repo,
        &[
            "log",
            "--no-color",
            "--no-show-signature",
            "--no-notes",
            "--format=%H %s",
            &range,
        ],
    )
    .await?;
    append_public_subjects(text, &subjects, &public);
    Ok(())
}

fn append_public_subjects(text: &mut String, subjects: &str, public: &HashSet<&str>) {
    let mut count = 0;
    let mut omitted = 0;
    let mut unpublished = 0;
    for line in subjects.lines() {
        let (id, subject) = line.split_once(' ').unwrap_or((line, ""));
        if !public.contains(id) {
            unpublished += 1;
            continue;
        }
        if count == 100 {
            omitted += 1;
            continue;
        }
        if count == 0 {
            text.push_str("Bundled public commits (newest first):\n");
        }
        count += 1;
        let clean: String = subject
            .chars()
            .filter(|c| !c.is_control())
            .take(240)
            .collect();
        text.push_str(&format!("{id} {clean}\n"));
    }
    if omitted > 0 {
        text.push_str(&format!("{omitted} older public commits omitted.\n"));
    }
    if unpublished > 0 {
        text.push_str(&format!(
            "{unpublished} commits not on canonical master excluded.\n"
        ));
    }
}

pub(super) async fn output(repo: Option<PathBuf>) -> Result<ToolOutput> {
    let mut text = match build::BuildManifest::load() {
        Ok(m) => match m.history.iter().max_by_key(|b| b.built_at) {
            Some(b) if sha(&b.full_hash) => format!(
                "Last successful local build: {} {}{}\n",
                b.built_at,
                b.full_hash,
                if b.dirty { " (dirty build)" } else { "" }
            ),
            None => "No successful local build recorded.\n".into(),
            _ => "Local build metadata invalid.\n".into(),
        },
        Err(_) => "Local build history unavailable.\n".into(),
    };
    if let Some(repo) = repo {
        match tokio::time::timeout(
            std::time::Duration::from_secs(45),
            public_history(&repo, &mut text),
        )
        .await
        {
            Ok(Err(e)) => text.push_str(&format!("Public history unavailable: {e}.\n")),
            Err(_) => text.push_str("Public history lookup timed out.\n"),
            Ok(Ok(())) => (),
        }
    } else {
        text.push_str("No jcode checkout available.\n");
    }
    Ok(ToolOutput::new(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unpublished_subjects_are_not_rendered() {
        let mut text = String::new();
        append_public_subjects(
            &mut text,
            "aaaa Public subject\nbbbb Private subject",
            &HashSet::from(["aaaa"]),
        );
        assert!(text.contains("aaaa Public subject"));
        assert!(text.contains("1 commits not on canonical master excluded"));
        assert!(!text.contains("Private") && !text.contains("bbbb"));
    }

    #[test]
    fn only_published_reachable_linux_selfdev_releases_qualify() {
        let id = "a".repeat(40);
        let ancestors = HashSet::from([id.as_str()]);
        let mut r = Release {
            draft: false,
            prerelease: true,
            published_at: Some(Utc::now()),
            tag_name: format!("selfdev-linux-{id}"),
        };
        assert!(r.reachable(&ancestors).is_some());
        assert!(r.reachable(&HashSet::new()).is_none());
        r.draft = true;
        assert!(r.reachable(&ancestors).is_none());
        r.draft = false;
        r.prerelease = false;
        assert!(r.reachable(&ancestors).is_none());
        r.prerelease = true;
        r.tag_name = id;
        assert!(r.reachable(&HashSet::new()).is_none());
    }

    #[test]
    fn release_pagination_stops_once_page_is_older_than_latest_found() {
        let now = Utc::now();
        let older = now - chrono::Duration::hours(2);
        let rows: Vec<Release> = (0..100)
            .map(|i| Release {
                draft: false,
                prerelease: true,
                published_at: Some(if i == 0 { now } else { older }),
                tag_name: format!("selfdev-linux-{}", "a".repeat(40)),
            })
            .collect();
        assert!(!should_stop_release_pagination(&rows, None));
        assert!(should_stop_release_pagination(
            &rows,
            Some(&(now, "a".repeat(40)))
        ));
        assert!(!should_stop_release_pagination(
            &rows,
            Some(&(older - chrono::Duration::hours(1), "a".repeat(40)))
        ));
        assert!(should_stop_release_pagination(&rows[..10], None));
    }
}

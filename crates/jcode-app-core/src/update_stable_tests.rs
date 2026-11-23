use super::*;

fn release(tag: &str, with_asset: bool, with_sums: bool) -> serde_json::Value {
    let mut assets = Vec::new();
    if with_asset {
        assets.push(serde_json::json!({
            "url": "https://api.github.com/repos/1jehuang/jcode/releases/assets/1",
            "id": 1, "node_id": "RA_x", "label": "", "state": "uploaded",
            "content_type": "application/gzip", "download_count": 0,
            "created_at": "2026-11-01T00:00:00Z", "updated_at": "2026-11-01T00:00:00Z",
            "name": format!("{}.tar.gz", get_asset_name()), "size": 100,
            "browser_download_url": format!("https://github.com/1jehuang/jcode/releases/download/{tag}/a.tar.gz")
        }));
    }
    if with_sums {
        assets.push(serde_json::json!({
            "id": 2, "name": "SHA256SUMS", "size": 64, "state": "uploaded",
            "browser_download_url": format!("https://github.com/1jehuang/jcode/releases/download/{tag}/SHA256SUMS")
        }));
    }
    serde_json::json!({
        "url": "https://api.github.com/repos/1jehuang/jcode/releases/1",
        "html_url": format!("https://github.com/1jehuang/jcode/releases/tag/{tag}"),
        "id": 1, "node_id": "RE_x", "tag_name": tag, "target_commitish": "master",
        "name": tag, "draft": false, "prerelease": false,
        "created_at": "2026-11-01T00:00:00Z", "published_at": "2026-11-01T00:00:00Z",
        "author": { "login": "1jehuang", "id": 1 },
        "tarball_url": null, "zipball_url": null, "body": "notes",
        "assets": assets
    })
}

fn parse(values: Vec<serde_json::Value>) -> Vec<ListedRelease> {
    serde_json::from_value(serde_json::Value::Array(values)).unwrap()
}

fn newer_than(current: &'static str) -> impl FnMut(&GitHubRelease) -> Result<bool> {
    move |r| Ok(version_is_newer(&r.tag_name, current))
}

#[test]
fn latest_without_platform_asset_falls_back_to_previous() {
    let releases = parse(vec![
        release("v1.2.0", false, true),
        release("v1.1.0", true, true),
    ]);
    let picked = select_installable_stable(releases, newer_than("1.0.0"), |_, _| true).unwrap();
    assert_eq!(picked.unwrap().tag_name, "v1.1.0");
}

#[test]
fn release_whose_sha256sums_lacks_entry_is_skipped() {
    let releases = parse(vec![
        release("v1.2.0", true, true),
        release("v1.1.0", true, true),
        release("v1.0.5", true, false),
    ]);
    let picked =
        select_installable_stable(releases, newer_than("1.0.0"), |r, _| r.tag_name != "v1.2.0")
            .unwrap();
    assert_eq!(picked.unwrap().tag_name, "v1.1.0");
}

#[test]
fn never_downgrades_when_nothing_newer_is_installable() {
    let releases = parse(vec![
        release("v1.2.0", false, true),
        release("v1.1.0", true, true),
        release("v1.0.0", true, true),
    ]);
    let picked = select_installable_stable(releases, newer_than("1.1.0"), |_, _| true).unwrap();
    assert!(picked.is_none());
}

#[test]
fn drafts_and_prereleases_are_ignored() {
    let mut draft = release("v1.3.0", true, true);
    draft["draft"] = true.into();
    let mut pre = release("v1.2.0", true, true);
    pre["prerelease"] = true.into();
    let releases = parse(vec![
        draft,
        pre,
        release("v1.0.0-preview.9", true, true),
        release("v1.1.0", true, true),
    ]);
    let picked = select_installable_stable(releases, newer_than("1.0.0"), |_, _| true).unwrap();
    assert_eq!(picked.unwrap().tag_name, "v1.1.0");
}

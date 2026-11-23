use super::*;

fn preview_fixture_with_published_at(tag: &str, published_at: &str) -> PreviewRelease {
    serde_json::from_value(serde_json::json!({
        "tag_name": tag,
        "html_url": "https://github.com/1jehuang/jcode/releases/test",
        "published_at": published_at,
        "draft": false,
        "prerelease": true,
        "assets": [
            { "name": format!("{}.tar.gz", get_asset_name()), "size": 100,
              "browser_download_url": "https://example.invalid/jcode.tar.gz" },
            { "name": "SHA256SUMS", "size": 100,
              "browser_download_url": "https://example.invalid/SHA256SUMS" }
        ]
    }))
    .unwrap()
}

fn preview_fixture(tag: &str) -> PreviewRelease {
    preview_fixture_with_published_at(tag, "2026-10-26T00:00:00Z")
}

#[test]
fn preview_discovery_accepts_selfdev_and_semver_preview_tags() {
    for tag in [
        "selfdev-linux-e8d3ca609344a00de3731cdb4b6a0bce3adb0510",
        "selfdev-macos-0123456789abcdef0123456789abcdef01234567",
        "v1.0.0-preview.1",
        "v1.0.0-preview.10",
        "v1.0.3-preview.2",
        "v2.0.0-preview.999",
    ] {
        assert!(preview_is_installable(&preview_fixture(tag)), "{tag}");
    }
    for tag in [
        "v1.0.0",
        "1.0.0-preview.1",
        "v1.0.0-preview.",
        "v1.0.0-preview.0",
        "v1.0.0-preview.01",
        "v1.0.0-preview.1+build",
        "v1.0.0-preview.1-extra",
        "v1.0.0-preview.-1",
        "selfdev-linux-short",
        "selfdev-linux-E8D3CA609344A00DE3731CDB4B6A0BCE3ADB0510",
        "selfdev--e8d3ca609344a00de3731cdb4b6a0bce3adb0510",
    ] {
        assert!(!preview_is_installable(&preview_fixture(tag)), "{tag}");
    }
    assert!(
        semver_preview_version("v1.0.0-preview.10") > semver_preview_version("v1.0.0-preview.9")
    );
}

#[test]
fn preview_discovery_rejects_draft_unpublished_and_incomplete_releases() {
    let valid = preview_fixture("v1.0.0-preview.2");
    assert!(preview_is_installable(&valid));
    let mut release = preview_fixture("v1.0.0-preview.2");
    release.draft = true;
    assert!(!preview_is_installable(&release));
    release.draft = false;
    release.prerelease = false;
    assert!(!preview_is_installable(&release));
    release.prerelease = true;
    release.release._published_at = None;
    assert!(!preview_is_installable(&release));
    let mut release = preview_fixture("v1.0.0-preview.2");
    release.release.assets.pop();
    assert!(!preview_is_installable(&release));
    let mut release = preview_fixture("v1.0.0-preview.2");
    release.release.assets[0].name = format!("{}.sha256", get_asset_name());
    assert!(!preview_is_installable(&release));
    assert!(platform_asset(&release.release).is_err());
    let mut release = preview_fixture("v1.0.0-preview.2");
    release.release.assets[0]._size = 0;
    assert!(!preview_is_installable(&release));
    let mut release = preview_fixture("v1.0.0-preview.2");
    release.release.assets[1].browser_download_url.clear();
    assert!(!preview_is_installable(&release));
}

#[test]
fn preview_identity_and_ordering_ignore_build_metadata() {
    assert!(
        !release_is_update_with("v1.0.0-preview.2", "1.0.0-preview.2+abc", true, || panic!(
            "release builds skip dev guard"
        ))
        .unwrap()
    );
    assert!(
        release_is_update_with("v1.0.0-preview.10", "1.0.0-preview.9+abc", true, || panic!(
            "release builds skip dev guard"
        ))
        .unwrap()
    );
    assert!(
        !release_is_update_with("v1.0.0-preview.9", "1.0.0-preview.10", true, || panic!(
            "older previews never install"
        ))
        .unwrap()
    );
    assert!(
        !release_is_update_with("v0.99.200", "1.0.0-preview.2", true, || panic!(
            "automatic stable checks never downgrade"
        ))
        .unwrap()
    );
    assert!(explicit_stable_is_update(
        "v0.99.200",
        "1.0.0-preview.2+abc",
        true
    ));
    assert!(!explicit_stable_is_update(
        "v0.99.200",
        "0.99.200+abc",
        true
    ));
    assert!(explicit_stable_is_update("v0.99.200", "0.99.200", false));
}

#[test]
fn preview_selection_uses_highest_valid_number_across_pages() {
    let mut latest = None;
    let mut incomplete = preview_fixture("v1.0.0-preview.30");
    incomplete.release.assets.clear();
    select_latest_preview(
        &mut latest,
        [
            preview_fixture("v1.0.0-preview.9"),
            incomplete,
            preview_fixture("v1.0.0-preview.10"),
            preview_fixture("v1.0.0"),
        ],
    );
    assert_eq!(latest.as_ref().unwrap().tag_name, "v1.0.0-preview.10");
    let mut draft = preview_fixture("v1.0.0-preview.50");
    draft.draft = true;
    select_latest_preview(
        &mut latest,
        [
            preview_fixture("v1.0.0-preview.2"),
            draft,
            preview_fixture("v1.0.0-preview.20"),
        ],
    );
    assert_eq!(latest.unwrap().tag_name, "v1.0.0-preview.20");
    let mut latest = None;
    select_latest_preview(&mut latest, [preview_fixture("v1.0.0")]);
    assert!(latest.is_none());
}

#[test]
fn preview_download_requires_checksums_even_if_called_directly() {
    for tag in [
        "v1.0.0-preview.1",
        "selfdev-linux-e8d3ca609344a00de3731cdb4b6a0bce3adb0510",
    ] {
        let mut candidate = preview_fixture(tag);
        candidate.release.assets.pop();
        let client = reqwest::blocking::Client::new();
        let asset = platform_asset(&candidate.release).unwrap();
        let error = verify_asset_checksum_if_available(&client, &candidate.release, asset, b"test")
            .expect_err("preview download must never silently skip checksums");
        assert!(error.to_string().contains("missing SHA256SUMS"), "{tag}");
    }
}

#[test]
fn selfdev_prerelease_selection_prefers_newest_published_at() {
    let mut latest = None;
    let older_preview =
        preview_fixture_with_published_at("v1.0.0-preview.424", "2026-11-12T05:27:21Z");
    let older_selfdev = preview_fixture_with_published_at(
        "selfdev-linux-2248281ef64759b991254c377ba2211d53e04868",
        "2026-11-22T04:12:41Z",
    );
    let newer_selfdev = preview_fixture_with_published_at(
        "selfdev-linux-e8d3ca609344a00de3731cdb4b6a0bce3adb0510",
        "2026-11-22T04:23:36Z",
    );
    select_latest_preview(&mut latest, [older_preview, older_selfdev, newer_selfdev]);
    assert_eq!(
        latest.unwrap().tag_name,
        "selfdev-linux-e8d3ca609344a00de3731cdb4b6a0bce3adb0510"
    );
}

#[test]
fn selfdev_prerelease_update_logic_handles_same_commit_explicit_and_auto() {
    let candidate = preview_fixture_with_published_at(
        "selfdev-linux-e8d3ca609344a00de3731cdb4b6a0bce3adb0510",
        "2026-11-22T04:23:36Z",
    )
    .release;

    // Already running the exact commit -> never reinstall.
    assert!(
        !preview_release_is_update_with(
            &candidate,
            true,
            "1.0.3-selfdev.1",
            "e8d3ca609",
            "2026-11-22 04:20:00 +0000",
            true,
            |_| panic!("same commit skips dev guard"),
        )
        .unwrap()
    );

    // Explicit switch from stable -> always installs.
    assert!(
        preview_release_is_update_with(
            &candidate,
            true,
            "1.0.3",
            "aaaaaaa",
            "2026-11-23 00:00:00 +0000",
            true,
            |_| panic!("explicit switch skips dev guard"),
        )
        .unwrap()
    );

    // Automatic preview check from older release -> installs.
    assert!(
        preview_release_is_update_with(
            &candidate,
            false,
            "1.0.3",
            "aaaaaaa",
            "2026-11-22 04:03:41 +0000",
            true,
            |_| panic!("release build skips dev guard"),
        )
        .unwrap()
    );

    // Automatic preview check from a newer commit -> does not downgrade.
    assert!(
        !preview_release_is_update_with(
            &candidate,
            false,
            "1.0.4",
            "bbbbbbb",
            "2026-11-22 05:00:00 +0000",
            true,
            |_| panic!("release build skips dev guard"),
        )
        .unwrap()
    );
}

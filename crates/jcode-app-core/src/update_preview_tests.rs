use super::*;

fn preview_fixture(tag: &str) -> PreviewRelease {
    serde_json::from_value(serde_json::json!({
        "tag_name": tag,
        "html_url": "https://github.com/1jehuang/jcode/releases/test",
        "published_at": "2026-10-26T00:00:00Z",
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

#[test]
fn preview_discovery_requires_exact_v1_preview_tags() {
    for tag in [
        "v1.0.0-preview.1",
        "v1.0.0-preview.10",
        "v1.0.0-preview.999",
    ] {
        assert!(preview_is_installable(&preview_fixture(tag)), "{tag}");
    }
    for tag in [
        "v1.0.0",
        "v1.0.1-preview.1",
        "v2.0.0-preview.1",
        "1.0.0-preview.1",
        "v1.0.0-preview.",
        "v1.0.0-preview.0",
        "v1.0.0-preview.01",
        "v1.0.0-preview.1+build",
        "v1.0.0-preview.1-extra",
        "v1.0.0-preview.-1",
    ] {
        assert!(!preview_is_installable(&preview_fixture(tag)), "{tag}");
    }
    assert!(preview_number("v1.0.0-preview.10") > preview_number("v1.0.0-preview.9"));
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
    let mut candidate = preview_fixture("v1.0.0-preview.1");
    candidate.release.assets.pop();
    let client = reqwest::blocking::Client::new();
    let asset = platform_asset(&candidate.release).unwrap();
    let error = verify_asset_checksum_if_available(&client, &candidate.release, asset, b"test")
        .expect_err("preview download must never silently skip checksums");
    assert!(error.to_string().contains("missing SHA256SUMS"));
}

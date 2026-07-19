// ImageRegion offsetting across body-cache reuse paths.
//
// Inline plan-graph diagrams and anchored raster images bake placeholder
// marker lines into the prepared body; `compute_image_regions` scans those
// into `ImageRegion`s and the incremental (`prepare_body_incremental`) and
// prepend (`prepare_body_prepended`) reuse paths must re-offset regions from
// the reused base so hit-testing and draw geometry stay correct. These tests
// pin:
//   (a) regions above/below an in-place mid-transcript edit boundary after a
//       prefix-reuse incremental rebuild,
//   (b) existing regions shifted by the head offset when compacted history is
//       prepended above an unchanged suffix,
//   (c) an edited message whose image render height changes gets a freshly
//       computed region rather than a stale-height one.

/// 40x2 PNG: width-dominant, fits in ~min rows (3 at 8x16 cells).
const IMG_REGION_WIDE_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAACgAAAACCAIAAAClqwlqAAAAEklEQVR4nGMQUDAYEMQwUBYDACXEHgHR+y+wAAAAAElFTkSuQmCC";
/// 2x200 PNG: height-dominant, needs many more rows than the wide image.
const IMG_REGION_TALL_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAADICAIAAABNp6ehAAAAGUlEQVR4nGMQUDAAIoZRapQapUapUYo+FAATfpYBTI1UPgAAAABJRU5ErkJggg==";

fn anchored_tool_image_with(
    tool_id: &str,
    png_b64: &str,
    label: &str,
) -> crate::session::RenderedImage {
    crate::session::RenderedImage {
        media_type: "image/png".to_string(),
        data: png_b64.to_string(),
        label: Some(label.to_string()),
        source: crate::session::RenderedImageSource::ToolResult {
            tool_name: "read".to_string(),
        },
        anchor: Some(crate::session::RenderedImageAnchor::ToolCall {
            id: tool_id.to_string(),
        }),
    }
}

fn inline_image_id_for(png_b64: &str) -> u64 {
    mermaid::inline_image_id("image/png", png_b64)
}

// ---------------------------------------------------------------------------
// compute_image_regions
// ---------------------------------------------------------------------------

#[test]
fn test_compute_image_regions_mermaid_placeholder_owns_blank_run() {
    let hash = 0x00AB_CDEF_1234_5678u64;
    let marker_text = mermaid::image_widget_placeholder_markdown(hash);
    let lines = vec![
        Line::from("text above"),
        Line::from(marker_text.trim_end_matches('\n').to_string()),
        Line::from(""),
        Line::from(""),
        Line::from("text below"),
    ];

    let regions = super::prepare::compute_image_regions(&lines);
    assert_eq!(regions.len(), 1);
    let region = &regions[0];
    assert_eq!(region.hash, hash);
    assert_eq!(region.abs_line_idx, 1, "region starts at the marker line");
    assert_eq!(region.height, 3, "marker plus the two-blank run");
    assert_eq!(region.end_line, 4, "exclusive end covers the blank run");
    assert_eq!(region.render, jcode_tui_messages::ImageRegionRender::Crop);
}

#[test]
fn test_compute_image_regions_inline_image_uses_marker_geometry() {
    let hash = 0xFEED_FACE_0000_0001u64;
    let mut lines = vec![Line::from("above")];
    // Marker + 3 blanks (rows = 4), plus an extra unrelated blank below it.
    lines.extend(mermaid::inline_image_placeholder_lines(hash, 4, 20));
    lines.push(Line::from(""));
    lines.push(Line::from("below"));

    let regions = super::prepare::compute_image_regions(&lines);
    assert_eq!(regions.len(), 1);
    let region = &regions[0];
    assert_eq!(region.hash, hash);
    assert_eq!(region.abs_line_idx, 1);
    assert_eq!(
        region.height, 4,
        "marker-encoded rows win when the blank run is long enough"
    );
    assert_eq!(region.end_line, 5);
    assert_eq!(region.width, 20);
    assert_eq!(region.render, jcode_tui_messages::ImageRegionRender::Fit);
}

#[test]
fn test_compute_image_regions_inline_image_clamps_to_available_blanks() {
    let hash = 0xFEED_FACE_0000_0002u64;
    // Marker claims 5 rows but only 1 blank line actually follows (e.g. a
    // wrapped or truncated placeholder). The region must not claim the
    // non-blank line below it.
    let placeholder = mermaid::inline_image_placeholder_lines(hash, 5, 24);
    let lines = vec![
        placeholder[0].clone(),
        Line::from(""),
        Line::from("non-blank content that must never be painted over"),
    ];

    let regions = super::prepare::compute_image_regions(&lines);
    assert_eq!(regions.len(), 1);
    assert_eq!(
        regions[0].height, 2,
        "clamped to marker line plus the single real blank"
    );
    assert_eq!(regions[0].end_line, 2);
}

// ---------------------------------------------------------------------------
// (a) prefix-reuse incremental rebuild re-offsets regions
// ---------------------------------------------------------------------------

/// Pure append: a region in the reused prefix keeps its absolute lines, the
/// appended message's region lands at the same lines a full rebuild computes.
#[test]
fn test_incremental_append_offsets_new_image_region_and_keeps_old() {
    let width = 80;
    let images = vec![
        anchored_tool_image_with("region-tool-a", IMG_REGION_WIDE_PNG_B64, "a.png"),
        anchored_tool_image_with("region-tool-b", BODY_ANCHOR_TINY_PNG_B64, "b.png"),
    ];
    let base_state = TestState {
        display_messages: vec![
            DisplayMessage::user("read the first screenshot"),
            DisplayMessage::tool("read a.png", read_tool_call("region-tool-a")),
        ],
        messages_version: 1,
        side_pane_images: images.clone(),
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };
    let grown_state = TestState {
        display_messages: vec![
            DisplayMessage::user("read the first screenshot"),
            DisplayMessage::tool("read a.png", read_tool_call("region-tool-a")),
            DisplayMessage::assistant("here is what I found in the image"),
            DisplayMessage::tool("read b.png", read_tool_call("region-tool-b")),
        ],
        messages_version: 2,
        side_pane_images: images,
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };

    let base = Arc::new(super::prepare::prepare_body(&base_state, width, false));
    // tool-b's anchor target does not exist in the base transcript, so it must
    // not have produced a body region yet.
    assert_eq!(base.image_regions.len(), 1, "base has only tool-a's region");
    let base_region = base.image_regions[0];
    assert_eq!(base_region.hash, inline_image_id_for(IMG_REGION_WIDE_PNG_B64));

    let k = super::prepare::matching_prefix_len(base.as_ref(), &grown_state.display_messages);
    assert_eq!(k, 2, "pure append: whole base is a matching prefix");
    let incremented = super::prepare::prepare_body_incremental(&grown_state, width, base, k);
    let full = super::prepare::prepare_body(&grown_state, width, false);
    assert_prepared_equivalent(&incremented, &full, "incremental_append_regions");

    assert_eq!(incremented.image_regions.len(), 2);
    // Region above the append boundary is untouched.
    assert_eq!(incremented.image_regions[0].abs_line_idx, base_region.abs_line_idx);
    assert_eq!(incremented.image_regions[0].end_line, base_region.end_line);
    assert_eq!(incremented.image_regions[0].hash, base_region.hash);
    // Region in the appended tail was shifted past the reused prefix.
    let prev_len = incremented.message_boundaries[k - 1].wrapped_len;
    assert!(
        incremented.image_regions[1].abs_line_idx >= prev_len,
        "appended region must sit below the reused prefix"
    );
    assert_eq!(
        incremented.image_regions[1].hash,
        inline_image_id_for(BODY_ANCHOR_TINY_PNG_B64)
    );
}

/// Mid-transcript in-place edit (the plan-graph upsert shape): the edited
/// message and everything below it are re-rendered, and regions below the edit
/// boundary get the recomputed offsets a full rebuild would produce.
#[test]
fn test_prefix_reuse_mid_edit_reoffsets_region_below_boundary() {
    let width = 72;
    let images = vec![anchored_tool_image_with(
        "region-tool-below",
        IMG_REGION_WIDE_PNG_B64,
        "below.png",
    )];
    let base_state = TestState {
        display_messages: vec![
            DisplayMessage::user("start"),
            DisplayMessage::swarm("Plan graph · test-swarm", "short plan"),
            DisplayMessage::tool("read below.png", read_tool_call("region-tool-below")),
            DisplayMessage::assistant("tail answer"),
        ],
        messages_version: 1,
        side_pane_images: images.clone(),
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };
    // The plan-graph message is upserted in place with taller content, moving
    // every line below it. The tool message's image region must move with it.
    let edited_state = TestState {
        display_messages: vec![
            DisplayMessage::user("start"),
            DisplayMessage::swarm(
                "Plan graph · test-swarm",
                // Markdown list items stay on separate rendered lines (single
                // newlines inside a paragraph would be merged into one line by
                // the compact plan-graph markdown renderer).
                "much longer plan\n\n- with extra graph rows\n- and another line\n- and one more",
            ),
            DisplayMessage::tool("read below.png", read_tool_call("region-tool-below")),
            DisplayMessage::assistant("tail answer"),
        ],
        messages_version: 2,
        side_pane_images: images,
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };

    let base = Arc::new(super::prepare::prepare_body(&base_state, width, false));
    assert_eq!(base.image_regions.len(), 1);
    let base_region = base.image_regions[0];

    let k = super::prepare::matching_prefix_len(base.as_ref(), &edited_state.display_messages);
    assert_eq!(k, 1, "prefix match stops at the edited plan-graph message");

    let mut reuse = base;
    super::prepare::truncate_prepared_to_boundary(Arc::make_mut(&mut reuse), k);
    let reuse = super::prepare::prepare_body_incremental(&edited_state, width, reuse, k);
    let full = super::prepare::prepare_body(&edited_state, width, false);
    assert_prepared_equivalent(&reuse, &full, "mid_edit_region_reoffset");

    assert_eq!(reuse.image_regions.len(), 1);
    assert!(
        reuse.image_regions[0].abs_line_idx > base_region.abs_line_idx,
        "taller upserted message must push the region below it down \
         (base {}, rebuilt {})",
        base_region.abs_line_idx,
        reuse.image_regions[0].abs_line_idx
    );
}

// ---------------------------------------------------------------------------
// (b) prepend (suffix reuse) shifts existing regions by the head offset
// ---------------------------------------------------------------------------

#[test]
fn test_prepend_shifts_suffix_image_regions_and_matches_full_build() {
    let width = 76;
    // tool-old's anchor target only appears once history is loaded; tool-new
    // exists in the base transcript. Same image set in both states, matching
    // the real flow where the body cache key pins the image signature.
    let images = vec![
        anchored_tool_image_with("prepend-tool-old", BODY_ANCHOR_TINY_PNG_B64, "old.png"),
        anchored_tool_image_with("prepend-tool-new", IMG_REGION_WIDE_PNG_B64, "new.png"),
    ];
    let base_state = TestState {
        display_messages: vec![
            DisplayMessage::user("recent question"),
            DisplayMessage::tool("read new.png", read_tool_call("prepend-tool-new")),
            DisplayMessage::assistant("recent answer about the screenshot"),
        ],
        messages_version: 1,
        side_pane_images: images.clone(),
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };
    // Older compacted history revealed above the unchanged tail; the revealed
    // head itself contains an image-bearing tool message.
    let prepended_state = TestState {
        display_messages: vec![
            DisplayMessage::system("─ older history loaded ─"),
            DisplayMessage::tool("read old.png", read_tool_call("prepend-tool-old")),
            DisplayMessage::user("recent question"),
            DisplayMessage::tool("read new.png", read_tool_call("prepend-tool-new")),
            DisplayMessage::assistant("recent answer about the screenshot"),
        ],
        messages_version: 2,
        side_pane_images: images,
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };

    let base = Arc::new(super::prepare::prepare_body(&base_state, width, false));
    assert_eq!(base.image_regions.len(), 1, "base has only the new-tool region");
    let base_region = base.image_regions[0];

    let s = super::prepare::matching_suffix_len(base.as_ref(), &prepended_state.display_messages);
    assert!(s >= 2, "tail must hash-match under the prepend (got {s})");
    // Reuse the tool+assistant tail; re-render the head (marker, revealed tool,
    // and the user prompt whose displayed number shifts under the prepend).
    let drop_msgs = 1usize;
    let head_count = prepended_state.display_messages.len() - (3 - drop_msgs);
    let cut_wrapped = base.message_boundaries[drop_msgs - 1].wrapped_len;

    let stitched =
        super::prepare::prepare_body_prepended(&prepended_state, width, base, drop_msgs, head_count)
            .unwrap_or_else(|_| panic!("prepend stitch should be sound for this shape"));
    let full = super::prepare::prepare_body(&prepended_state, width, false);
    assert_prepared_equivalent(&stitched, &full, "prepend_region_shift");

    assert_eq!(stitched.image_regions.len(), 2);
    // Head region (revealed history) comes first and belongs to tool-old.
    assert_eq!(
        stitched.image_regions[0].hash,
        inline_image_id_for(BODY_ANCHOR_TINY_PNG_B64)
    );
    // The suffix region kept its identity and was shifted by exactly
    // `head_len - cut_wrapped`, preserving its height.
    let suffix_region = stitched.image_regions[1];
    assert_eq!(suffix_region.hash, base_region.hash);
    assert_eq!(suffix_region.height, base_region.height);
    let head_len = stitched.message_boundaries[head_count - 1].wrapped_len;
    assert_eq!(
        suffix_region.abs_line_idx,
        base_region.abs_line_idx - cut_wrapped + head_len,
        "suffix region must shift by head_len - cut_wrapped"
    );
    assert_eq!(
        suffix_region.end_line,
        base_region.end_line - cut_wrapped + head_len
    );
}

// ---------------------------------------------------------------------------
// (c) edited message whose image height changes gets a recomputed region
// ---------------------------------------------------------------------------

/// When an upserted message's rendered image changes height (e.g. a plan-graph
/// mermaid render growing), the prefix-reuse rebuild must recompute the region
/// from the new placeholder geometry instead of reusing the stale height.
#[test]
fn test_prefix_reuse_edit_recomputes_region_height_not_stale() {
    let width = 80;
    let base_state = TestState {
        display_messages: vec![
            DisplayMessage::user("show the diagram"),
            DisplayMessage::tool("read graph.png", read_tool_call("region-tool-grow")),
            DisplayMessage::assistant("rendered"),
        ],
        messages_version: 1,
        side_pane_images: vec![anchored_tool_image_with(
            "region-tool-grow",
            IMG_REGION_WIDE_PNG_B64,
            "graph.png",
        )],
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };
    // The producing message is edited in place and its image is replaced by a
    // much taller render (same anchor id, new payload => new geometry).
    let edited_state = TestState {
        display_messages: vec![
            DisplayMessage::user("show the diagram"),
            DisplayMessage::tool("read graph-v2.png", read_tool_call("region-tool-grow")),
            DisplayMessage::assistant("rendered"),
        ],
        messages_version: 2,
        side_pane_images: vec![anchored_tool_image_with(
            "region-tool-grow",
            IMG_REGION_TALL_PNG_B64,
            "graph-v2.png",
        )],
        pin_images: true,
        inline_images_visible: true,
        ..Default::default()
    };

    let base = Arc::new(super::prepare::prepare_body(&base_state, width, false));
    assert_eq!(base.image_regions.len(), 1);
    let base_region = base.image_regions[0];

    let k = super::prepare::matching_prefix_len(base.as_ref(), &edited_state.display_messages);
    assert_eq!(k, 1, "prefix match stops at the edited tool message");
    let mut reuse = base;
    super::prepare::truncate_prepared_to_boundary(Arc::make_mut(&mut reuse), k);
    let reuse = super::prepare::prepare_body_incremental(&edited_state, width, reuse, k);
    let full = super::prepare::prepare_body(&edited_state, width, false);
    assert_prepared_equivalent(&reuse, &full, "edit_region_height");

    assert_eq!(reuse.image_regions.len(), 1);
    let rebuilt = reuse.image_regions[0];
    assert_eq!(
        rebuilt.hash,
        inline_image_id_for(IMG_REGION_TALL_PNG_B64),
        "region must point at the new image, not the stale one"
    );
    assert_eq!(rebuilt.height, full.image_regions[0].height);
    // The tall image must occupy more rows than the wide one did; if the
    // rebuild had reused the stale region this would fail.
    assert!(
        rebuilt.height > base_region.height,
        "recomputed height ({}) must exceed the stale base height ({})",
        rebuilt.height,
        base_region.height
    );
}

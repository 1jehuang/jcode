use super::*;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::{Terminal, backend::TestBackend};

fn isolated(test: impl FnOnce()) {
    let _guard = IMAGE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    runtime::with_classic_kitty_for_test(|| {
        clear_image_state();
        let _ = take_terminal_image_cleanup_payload();
        test();
        clear_image_state();
        let _ = take_terminal_image_cleanup_payload();
    });
}

fn seed(hash: u64) {
    let source = DynamicImage::new_rgba8(80, 80);
    viewport_render::ensure_kitty_viewport_state(
        hash,
        Path::new("/synthetic.png"),
        &source,
        100,
        (10, 10),
    )
    .expect("classic image state");
}

fn output(buf: &Buffer) -> String {
    buf.content.iter().map(|cell| cell.symbol()).collect()
}

#[test]
fn public_image_frames_compose_two_origin_local_panes_with_one_tail() {
    isolated(|| {
        force_test_kitty_picker();
        let path = std::env::temp_dir().join(format!(
            "jcode-classic-composition-{}.png",
            std::process::id()
        ));
        DynamicImage::new_rgba8(80, 80).save(&path).unwrap();
        let hash = register_external_image(&path, 80, 80);
        let mut previous = Buffer::empty(Rect::new(0, 0, 24, 6));
        for frame_index in 0..2 {
            crate::begin_terminal_image_frame();
            let mut composed = Buffer::empty(*previous.area());
            for offset in [0, 12] {
                let pane = crate::with_image_frame_offset(offset, 0, || {
                    let mut terminal = Terminal::new(TestBackend::new(12, 6)).unwrap();
                    let completed = terminal
                        .draw(|frame| {
                            crate::begin_terminal_image_frame();
                            assert!(crate::render_image_widget_fit_stable(
                                hash,
                                Rect::new(1, 1, 4, 3),
                                frame.buffer_mut(),
                                4,
                                3,
                                0,
                                false,
                                false,
                            ));
                            assert!(!crate::finish_terminal_image_frame(frame.buffer_mut()));
                        })
                        .unwrap();
                    completed.buffer.clone()
                });
                assert!(output(&pane).contains("jcode-image="));
                assert!(!output(&pane).contains("a=p,"));
                for y in 0..6 {
                    for x in 0..12 {
                        *composed.cell_mut((x + offset, y)).unwrap() =
                            pane.cell((x, y)).unwrap().clone();
                    }
                }
            }
            assert!(crate::finish_terminal_image_frame(&mut composed));
            let mut bytes = Vec::new();
            CrosstermBackend::new(&mut bytes)
                .draw(previous.diff(&composed).into_iter())
                .unwrap();
            let stream = String::from_utf8(bytes).unwrap();
            assert_eq!(stream.matches("a=p,").count(), 2);
            assert!(stream.contains("\x1b[2;2H\x1b_Ga=p,"));
            assert!(stream.contains("\x1b[2;14H\x1b_Ga=p,"));
            assert_eq!(
                stream.matches("a=t,").count(),
                usize::from(frame_index == 0)
            );
            assert_eq!(
                stream.matches("a=d,d=i,").count(),
                if frame_index == 0 { 0 } else { 2 }
            );
            if frame_index != 0 {
                assert!(stream.rfind("a=d,d=i,").unwrap() < stream.find("a=p,").unwrap());
            }
            assert!(!stream.contains("jcode-image="));
            assert!(!stream.contains("U=1"));
            previous = composed;
        }
        fs::remove_file(path).unwrap();
    });
}

#[test]
fn image_frame_offsets_compose_and_restore_after_panics() {
    isolated(|| {
        seed(11);
        begin_terminal_image_frame();
        let mut local = Buffer::empty(Rect::new(0, 0, 8, 4));
        crate::with_image_frame_offset(3, 2, || {
            crate::with_image_frame_offset(5, 2, || {
                begin_terminal_image_frame();
                render_cell_viewport(11, Rect::new(1, 1, 3, 2), &mut local, 0, 0, 3, 2);
                assert!(!finish_terminal_image_frame(&mut local));
            });
        });
        let mut composed = Buffer::empty(Rect::new(0, 0, 20, 12));
        for y in 0..4 {
            for x in 0..8 {
                *composed.cell_mut((x + 8, y + 4)).unwrap() = local.cell((x, y)).unwrap().clone();
            }
        }
        assert!(finish_terminal_image_frame(&mut composed));
        assert!(output(&composed).contains("\x1b[6;10H\x1b_Ga=p,"));

        assert!(
            std::panic::catch_unwind(|| {
                crate::with_image_frame_offset(50, 50, || {
                    panic!("synthetic nested render failure")
                });
            })
            .is_err()
        );
        begin_terminal_image_frame();
        let mut normal = Buffer::empty(Rect::new(0, 0, 8, 4));
        render_cell_viewport(11, Rect::new(1, 1, 3, 2), &mut normal, 0, 0, 3, 2);
        assert!(finish_terminal_image_frame(&mut normal));
        assert!(output(&normal).contains("\x1b[2;2H\x1b_Ga=p,"));
    });
}

#[test]
fn translated_image_outside_composed_bounds_is_not_placed() {
    isolated(|| {
        seed(12);
        begin_terminal_image_frame();
        let mut local = Buffer::empty(Rect::new(0, 0, 4, 4));
        crate::with_image_frame_offset(u16::MAX, 0, || {
            render_cell_viewport(12, Rect::new(1, 1, 3, 2), &mut local, 0, 0, 3, 2);
        });
        let mut composed = Buffer::empty(Rect::new(0, 0, 12, 6));
        assert!(finish_terminal_image_frame(&mut composed));
        assert!(!output(&composed).contains("a=p,"));
        assert!(!output(&composed).contains("jcode-image="));
    });
}

#[test]
fn classic_placements_move_scroll_and_disappear_without_reuploading_pixels() {
    isolated(|| {
        seed(1);
        let mut first = Buffer::empty(Rect::new(0, 0, 20, 10));
        begin_terminal_image_frame();
        assert!(render_cell_viewport(
            1,
            Rect::new(2, 1, 4, 3),
            &mut first,
            1,
            2,
            4,
            3
        ));
        assert!(finish_terminal_image_frame(&mut first));
        let emitted = output(&first);
        assert!(emitted.contains("a=t,f=100"));
        assert!(emitted.contains("x=10,y=20,w=40,h=30,c=4,r=3,C=1,q=2"));
        assert!(emitted.contains("\x1b[2;3H"));
        assert!(!emitted.contains("U=1"));
        assert!(!emitted.contains('\u{10EEEE}'));
        assert!(!emitted.contains("jcode-image="));

        let mut moved = Buffer::empty(*first.area());
        begin_terminal_image_frame();
        assert!(render_cell_viewport(
            1,
            Rect::new(3, 2, 4, 2),
            &mut moved,
            0,
            6,
            4,
            2
        ));
        finish_terminal_image_frame(&mut moved);
        let emitted = output(&moved);
        assert!(emitted.contains("a=d,d=i,"));
        assert!(emitted.contains("\x1b[3;4H"));
        assert!(emitted.contains("x=0,y=60,w=40,h=20,c=4,r=2"));
        assert!(!emitted.contains("a=t,"));
        assert!(
            !emitted.contains("d=I"),
            "scrolling must keep the pixel allocation"
        );

        let mut removed = Buffer::empty(*first.area());
        begin_terminal_image_frame();
        assert!(finish_terminal_image_frame(&mut removed));
        assert!(output(&removed).contains("a=d,d=i,"));
        assert!(!output(&removed).contains("a=p,"));
        begin_terminal_image_frame();
        assert!(!finish_terminal_image_frame(&mut Buffer::empty(
            *first.area()
        )));
    });
}

#[test]
fn classic_frames_use_independent_placements_for_the_same_image() {
    isolated(|| {
        seed(2);
        for frame_index in 0..2 {
            begin_terminal_image_frame();
            let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
            assert!(render_cell_viewport(
                2,
                Rect::new(1, 1, 3, 2),
                &mut buf,
                0,
                0,
                3,
                2
            ));
            assert!(render_cell_viewport(
                2,
                Rect::new(8, 4, 3, 2),
                &mut buf,
                3,
                2,
                3,
                2
            ));
            finish_terminal_image_frame(&mut buf);
            let emitted = output(&buf);
            assert_eq!(emitted.matches("a=p,").count(), 2);
            assert!(emitted.contains(",p=1,x="));
            assert!(emitted.contains(",p=2,x="));
            assert_eq!(
                emitted.matches("a=t,").count(),
                usize::from(frame_index == 0)
            );
        }
    });
}

#[test]
fn classic_overlay_hides_placement_and_strips_internal_markers() {
    isolated(|| {
        seed(3);
        begin_terminal_image_frame();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 6));
        render_cell_viewport(3, Rect::new(1, 1, 5, 3), &mut buf, 0, 0, 5, 3);
        buf.cell_mut((2, 2)).unwrap().set_symbol("overlay");
        finish_terminal_image_frame(&mut buf);
        let emitted = output(&buf);
        assert!(!emitted.contains("a=p,"));
        assert!(!emitted.contains("jcode-image="));
        assert!(emitted.contains("overlay"));
    });
}

#[test]
fn classic_crop_clips_source_and_destination_to_buffer_bounds() {
    isolated(|| {
        seed(4);
        begin_terminal_image_frame();
        let mut buf = Buffer::empty(Rect::new(2, 2, 4, 3));
        assert!(render_crop(
            4,
            Rect::new(0, 0, 10, 10),
            &mut buf,
            (0, 0, 100, 100)
        ));
        finish_terminal_image_frame(&mut buf);
        let emitted = output(&buf);
        assert!(emitted.contains("\x1b[3;3H"));
        assert!(emitted.contains("x=20,y=20,w=40,h=30,c=4,r=3"));
    });
}

#[test]
fn classic_frame_tail_follows_text_and_survives_identical_buffer_diffs() {
    isolated(|| {
        seed(5);
        let mut previous = Buffer::empty(Rect::new(0, 0, 12, 6));
        for _ in 0..3 {
            begin_terminal_image_frame();
            let mut current = Buffer::empty(*previous.area());
            render_cell_viewport(5, Rect::new(1, 1, 3, 2), &mut current, 0, 0, 3, 2);
            // Last logical cell is a wide glyph, not its continuation cell.
            current.set_string(10, 5, "界", Style::default());
            finish_terminal_image_frame(&mut current);
            let mut bytes = Vec::new();
            CrosstermBackend::new(&mut bytes)
                .draw(previous.diff(&current).into_iter())
                .unwrap();
            let stream = String::from_utf8(bytes).unwrap();
            assert!(
                stream.contains("a=p,"),
                "placement must survive frame diffing"
            );
            assert!(stream.find('界').unwrap() < stream.find("a=p,").unwrap());
            assert!(!stream.contains("jcode-image="));
            previous = current;
        }
    });
}

#[test]
fn classic_all_skip_frame_preserves_one_shot_upload_for_recovery() {
    isolated(|| {
        seed(6);
        begin_terminal_image_frame();
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 4));
        render_cell_viewport(6, Rect::new(1, 1, 3, 2), &mut buf, 0, 0, 3, 2);
        for cell in &mut buf.content {
            cell.set_skip(true);
        }
        assert!(!finish_terminal_image_frame(&mut buf));
        assert_eq!(FRAME.lock().unwrap().pending.len(), 1);
        for cell in &mut buf.content {
            cell.set_skip(false);
        }
        assert!(finish_terminal_image_frame(&mut buf));
        assert!(output(&buf).contains("a=t,"));
        clear_image_state();
        let cleanup = take_terminal_image_cleanup_payload();
        assert!(cleanup.contains("a=d,d=i,"));
        assert!(cleanup.contains("a=d,d=I,"));
    });
}

#[cfg(feature = "renderer")]
#[test]
fn classic_content_invalidation_reclaims_all_geometry_variants() {
    isolated(|| {
        let font = (10, 10);
        let original = state_hash(7, None, font);
        let fitted = state_hash(7, Some((4, 4)), font);
        seed(original);
        viewport_render::ensure_kitty_fit_state(
            fitted,
            Path::new("/synthetic.png"),
            &DynamicImage::new_rgba8(80, 80),
            4,
            4,
            font,
        )
        .unwrap();
        seed(state_hash(8, None, font));
        let mut cache = KITTY_VIEWPORT_STATE.lock().unwrap();
        cache.take_pending_transmit(original).unwrap();
        cache.take_pending_transmit(fitted).unwrap();
        cache.remove(&7);
        let original_retained = cache.entries.contains_key(&original);
        let fitted_retained = cache.entries.contains_key(&fitted);
        let unrelated_retained = cache.entries.contains_key(&state_hash(8, None, font));
        drop(cache);
        assert!(!original_retained);
        assert!(!fitted_retained);
        assert!(unrelated_retained);
        assert_eq!(take_kitty_delete_ids().len(), 2);
    });
}

#[test]
fn classic_eviction_before_flush_does_not_recreate_an_orphaned_allocation() {
    isolated(|| {
        seed(9);
        begin_terminal_image_frame();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 6));
        render_cell_viewport(9, Rect::new(1, 1, 4, 3), &mut buf, 0, 0, 4, 3);
        KITTY_VIEWPORT_STATE.lock().unwrap().clear();
        finish_terminal_image_frame(&mut buf);
        let emitted = output(&buf);
        assert!(emitted.contains("a=d,d=I,"));
        assert!(!emitted.contains("a=t,"));
        assert!(!emitted.contains("a=p,"));
        assert!(!emitted.contains("jcode-image="));
    });
}

#[test]
fn classic_abandoned_frame_rebuilds_its_unsent_upload() {
    isolated(|| {
        seed(10);
        begin_terminal_image_frame();
        render_cell_viewport(
            10,
            Rect::new(1, 1, 4, 3),
            &mut Buffer::empty(Rect::new(0, 0, 10, 6)),
            0,
            0,
            4,
            3,
        );
        begin_terminal_image_frame();
        assert!(KITTY_VIEWPORT_STATE.lock().unwrap().entries.is_empty());
        seed(10);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 6));
        render_cell_viewport(10, Rect::new(1, 1, 4, 3), &mut buf, 0, 0, 4, 3);
        finish_terminal_image_frame(&mut buf);
        assert!(output(&buf).contains("a=t,"));
    });
}

#[test]
fn all_public_kitty_widgets_use_classic_placements_and_keep_variants_warm() {
    isolated(|| {
        force_test_kitty_picker();
        let path =
            std::env::temp_dir().join(format!("jcode-classic-widgets-{}.png", std::process::id()));
        DynamicImage::new_rgba8(160, 160).save(&path).unwrap();
        let hash = register_external_image(&path, 160, 160);
        for index in 0..2 {
            begin_terminal_image_frame();
            let mut buf = Buffer::empty(Rect::new(0, 0, 80, 40));
            assert!(render_image_widget(hash, Rect::new(0, 0, 15, 6), &mut buf, false, true) > 0);
            assert!(
                render_image_widget_fit(hash, Rect::new(20, 0, 15, 6), &mut buf, false, false) > 0
            );
            assert!(render_image_widget_scale(hash, Rect::new(40, 0, 18, 8), &mut buf, false) > 0);
            assert!(render_image_widget_fit_stable(
                hash,
                Rect::new(0, 12, 15, 6),
                &mut buf,
                15,
                8,
                2,
                false,
                false
            ));
            assert!(
                render_image_widget_viewport_precise(
                    hash,
                    Rect::new(22, 12, 15, 6),
                    &mut buf,
                    2,
                    3,
                    1000,
                    false
                ) > 0
            );
            assert!(
                render_image_widget_viewport(
                    hash,
                    Rect::new(45, 12, 15, 6),
                    &mut buf,
                    1,
                    2,
                    150,
                    false
                ) > 0
            );
            finish_terminal_image_frame(&mut buf);
            let emitted = output(&buf);
            assert_eq!(emitted.matches("a=p,").count(), 6);
            assert!(!emitted.contains("U=1"));
            assert!(!emitted.contains('\u{10EEEE}'));
            assert!(!emitted.contains("jcode-image="));
            if index == 1 {
                assert!(
                    !emitted.contains("a=t,"),
                    "all simultaneous geometry variants should remain cached"
                );
            }
        }
        fs::remove_file(path).unwrap();
    });
}

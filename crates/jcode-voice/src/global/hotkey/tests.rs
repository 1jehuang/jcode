use super::*;
fn temp() -> PathBuf {
    std::env::temp_dir().join(format!(
        "jcode-voice-test-{}-{}-{}",
        std::process::id(),
        now().unwrap(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
#[test]
fn decoder_chords_repeat_and_release() {
    let mut d = Decoder::default();
    assert_eq!(d.event(1, F23 as u16, 1), None);
    d.event(1, F23 as u16, 0);
    // Hardware order: synthetic Meta, then Shift, then F23.
    d.event(1, 125, 1);
    d.event(1, 42, 1);
    assert_eq!(d.event(1, F23 as u16, 1), Some(Edge::Press));
    assert_eq!(d.event(1, F23 as u16, 2), None);
    assert_eq!(d.event(1, F23 as u16, 1), None);
    assert_eq!(d.event(1, 42, 0), None);
    assert_eq!(d.event(1, F23 as u16, 0), Some(Edge::Release));
    d.event(1, 125, 0);
    assert_eq!(d.event(1, ASSISTANT as u16, 1), Some(Edge::Press));
    assert_eq!(d.event(1, ASSISTANT as u16, 0), Some(Edge::Release));
    assert_eq!(d.event(1, 30, 1), None);
}
#[test]
fn shift_before_copilot_spawns_but_synthetic_shift_holds() {
    // Copilot alone: synthetic Meta, then Shift, then F23.
    let mut d = Decoder::default();
    d.event(1, 125, 1);
    d.event(1, 42, 1);
    assert_eq!(d.event(1, F23 as u16, 1), Some(Edge::Press));
    assert_eq!(d.event(1, F23 as u16, 0), Some(Edge::Release));
    // User Shift first. The kernel drops the synthetic duplicate Shift.
    let mut d = Decoder::default();
    d.event(1, 42, 1);
    d.event(1, 125, 1);
    assert_eq!(d.event(1, F23 as u16, 1), Some(Edge::Spawn));
    assert_eq!(d.event(1, 42, 0), None);
    assert_eq!(d.event(1, F23 as u16, 0), Some(Edge::Release));
    // Right Shift is never synthesized.
    let mut d = Decoder::default();
    d.event(1, 125, 1);
    d.event(1, 42, 1);
    d.event(1, 54, 1);
    assert_eq!(d.event(1, F23 as u16, 1), Some(Edge::Spawn));
}
#[test]
fn spawned_window_adopts_a_held_key_or_falls_back_to_a_tap() {
    for held in [true, false] {
        let (mut listener, mut writer, dir) = fake_listener();
        listener.capture = None;
        if held {
            send(&mut writer, 1, 42, 1);
            send(&mut writer, 1, 125, 1);
            // Lost to the opener's routing lock, still physically down.
            let routing = Lock::take(&dir.join("routing.lock")).unwrap();
            send(&mut writer, 1, F23 as u16, 1);
            assert!(listener.poll().is_empty());
            listener.pending = None;
            drop(routing);
        }
        listener.adopt_held_key();
        assert_eq!(
            listener.poll(),
            vec![if held { Edge::Press } else { Edge::Tap }]
        );
        assert!(listener.poll().is_empty(), "adoption happens once");
        if held {
            send(&mut writer, 1, F23 as u16, 0);
            assert_eq!(listener.poll(), vec![Edge::Release]);
        }
        drop(listener);
        fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn held_at_open_is_ignored() {
    let mut bits = [0; KEY_BYTES];
    for k in KEYS {
        bits[k / 8] |= 1 << (k % 8);
    }
    let mut d = Decoder::snapshot(&bits);
    assert_eq!(d.event(1, ASSISTANT as u16, 1), None);
    assert_eq!(d.event(1, ASSISTANT as u16, 0), None);
    for key in [29, 97, 56, 100] {
        d.event(1, key, 0);
    }
    assert_eq!(d.event(1, ASSISTANT as u16, 1), Some(Edge::Press));
}
#[test]
fn global_chord_rejects_control_alt_and_partial_synthetic_modifiers() {
    for key in [29, 97, 56, 100, 42, 125] {
        let mut d = Decoder::default();
        d.event(1, key, 1);
        assert_eq!(d.event(1, ASSISTANT as u16, 1), None);
    }
    for path in [
        "/dev/input/event3",
        "/dev/input/by-path/platform-i8042-serio-0-event-kbd",
        "/dev/input/by-id/usb-keyboard-event-kbd",
    ] {
        assert!(allowed_device_path(Path::new(path)));
    }
    for path in [
        "/dev/input/event",
        "/dev/input/mouse0",
        "/dev/input/by-path/../event3",
        "/dev/input/by-id/device",
        "event3",
    ] {
        assert!(!allowed_device_path(Path::new(path)));
    }
}
#[test]
fn newest_focus_and_stable_lease() {
    let dir = temp();
    let mut a = Registration::new(dir.clone()).unwrap();
    let mut b = Registration::new(dir.clone()).unwrap();
    a.update(Some(true), true).unwrap();
    let lease = a.capture(now().unwrap()).unwrap().unwrap();
    b.update(Some(true), true).unwrap();
    assert!(a.capture(now().unwrap()).unwrap().is_none());
    assert!(b.capture(now().unwrap()).unwrap().is_none());
    drop(lease);
    assert!(b.capture(now().unwrap()).unwrap().is_some());
    // Repeated active heartbeats must not steal newer focus.
    a.update(Some(true), true).unwrap();
    assert!(a.capture(now().unwrap()).unwrap().is_none());
    let b_path = b.path.clone();
    drop(b);
    assert!(!b_path.exists());
    assert!(a.capture(now().unwrap()).unwrap().is_some());
    drop(a);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn stale_and_unready_registrations_are_excluded() {
    let dir = temp();
    let mut a = Registration::new(dir.clone()).unwrap();
    let mut b = Registration::new(dir.clone()).unwrap();
    a.update(Some(true), true).unwrap();
    b.update(Some(true), false).unwrap();
    assert!(a.capture(now().unwrap()).unwrap().is_some());
    fs::write(&b.path, format!("{} 0 1\n", u64::MAX)).unwrap();
    assert!(a.capture(now().unwrap()).unwrap().is_some());
    drop(a);
    drop(b);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn delayed_press_cannot_recapture_after_focus_changes() {
    let dir = temp();
    let mut a = Registration::new(dir.clone()).unwrap();
    let mut b = Registration::new(dir.clone()).unwrap();
    a.update(Some(true), true).unwrap();
    let stamp = now().unwrap();
    drop(a.capture(stamp).unwrap().unwrap());
    b.update(Some(true), true).unwrap();
    assert!(b.capture(stamp).unwrap().is_none());
    assert!(b.capture(stamp - 1).unwrap().is_none());
    assert!(b.capture(stamp + 1).unwrap().is_some());
    drop(a);
    drop(b);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn capability_bitmap_and_native_abi() {
    let mut words = [0usize; KEY_BYTES / size_of::<usize>()];
    for key in [F23, ASSISTANT] {
        words[key / usize::BITS as usize] |= 1 << (key % usize::BITS as usize);
    }
    let text = words
        .iter()
        .rev()
        .map(|w| format!("{w:x}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(advertised(&text, F23).unwrap());
    assert!(advertised(&text, ASSISTANT).unwrap());
    assert!(!advertised(&text, 30).unwrap());
    assert!(advertised("invalid", F23).is_err());
    assert_eq!(size_of::<InputEvent>(), size_of::<libc::timeval>() + 8);
    assert_eq!(size_of::<InputMask>(), 16);
    assert_eq!(request(true, 0x93, 16), 0x40104593);
}
fn fake_listener() -> (Listener, File, PathBuf) {
    use std::os::fd::FromRawFd;
    let dir = temp();
    let mut fds = [-1; 2];
    // SAFETY: writable two-fd array. Only a private anonymous pipe, no devices.
    assert_eq!(
        unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) },
        0
    );
    let (read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
    let mut listener = Listener {
        registration: Registration::new(dir.clone()).unwrap(),
        allowlist: Vec::new(),
        eligible: true,
        devices: BTreeMap::from([(
            PathBuf::from("fixture"),
            Device {
                file: read,
                decoder: Decoder::default(),
            },
        )]),
        capture: None,
        pending: None,
        adopt_until: None,
        next_scan: Instant::now() + Duration::from_secs(1000),
        healthy: true,
        registration_ok: false,
    };
    listener.set_active(true);
    (listener, write, dir)
}
fn send(file: &mut File, kind: u16, code: u16, value: i32) {
    let event = InputEvent {
        time: libc::timeval {
            tv_sec: 1,
            tv_usec: 0,
        },
        kind,
        code,
        value,
    };
    // SAFETY: initialized native ABI struct, read-only for its exact size.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            (&event as *const InputEvent).cast::<u8>(),
            size_of::<InputEvent>(),
        )
    };
    file.write_all(bytes).unwrap();
}
#[test]
fn dropped_events_resync_keeps_a_held_key_and_releases_a_lifted_one() {
    for still_held in [true, false] {
        let (mut listener, mut writer, dir) = fake_listener();
        send(&mut writer, 1, ASSISTANT as u16, 1);
        assert_eq!(listener.poll(), vec![Edge::Press]);
        let decoder = &mut listener
            .devices
            .get_mut(Path::new("fixture"))
            .unwrap()
            .decoder;
        let mut held = [0u8; KEY_BYTES];
        if still_held {
            held[ASSISTANT / 8] |= 1 << (ASSISTANT % 8);
        }
        assert_eq!(decoder.resync(&held), still_held);
        assert_eq!(decoder.trigger.is_some(), still_held);
        drop(listener);
        fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn poll_dropped_disconnect_deadline_and_drop() {
    for reason in 0..4 {
        let (mut listener, mut writer, dir) = fake_listener();
        assert!(listener.ready());
        send(&mut writer, 1, ASSISTANT as u16, 1);
        assert_eq!(listener.poll(), vec![Edge::Press]);
        assert!(Lock::take(&dir.join("capture.lock")).is_err());
        match reason {
            0 => send(&mut writer, 0, 3, 0),
            1 => drop(writer),
            2 => listener.capture.as_mut().unwrap().1 = Instant::now() - DEADLINE,
            _ => {
                let path = listener.registration.path.clone();
                drop(listener);
                assert!(!path.exists());
                assert!(Lock::take(&dir.join("capture.lock")).is_ok());
                fs::remove_dir_all(dir).unwrap();
                continue;
            }
        }
        assert_eq!(listener.poll(), vec![Edge::Cancel]);
        assert!(listener.poll().is_empty());
        assert!(Lock::take(&dir.join("capture.lock")).is_ok());
        drop(listener);
        fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn eligibility_cancels_but_physical_key_up_releases() {
    for eligible in [false, true] {
        let (mut listener, mut writer, dir) = fake_listener();
        send(&mut writer, 1, ASSISTANT as u16, 1);
        assert_eq!(listener.poll(), vec![Edge::Press]);
        if eligible {
            send(&mut writer, 1, ASSISTANT as u16, 0);
            assert_eq!(listener.poll(), vec![Edge::Release]);
        } else {
            listener.set_eligible(false);
            assert!(!listener.ready());
            assert_eq!(listener.poll(), vec![Edge::Cancel]);
            listener.set_eligible(true);
            send(&mut writer, 1, ASSISTANT as u16, 2);
            assert!(listener.poll().is_empty());
        }
        drop(listener);
        fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn registry_failure_cancels_and_invalid_allowlist_fails() {
    let (mut listener, mut writer, dir) = fake_listener();
    send(&mut writer, 1, ASSISTANT as u16, 1);
    assert_eq!(listener.poll(), vec![Edge::Press]);
    fs::remove_file(dir.join("routing.lock")).unwrap();
    fs::create_dir(dir.join("routing.lock")).unwrap();
    listener.registration.published = None;
    assert_eq!(listener.poll(), vec![Edge::Cancel]);
    assert!(!listener.ready());
    drop(listener);
    fs::remove_dir_all(dir).unwrap();
    // Rejected before looking up XDG_RUNTIME_DIR or opening anything.
    assert!(Listener::new(&[PathBuf::from("/dev/input/mouse0")]).is_err());
    assert!(Listener::new(&[PathBuf::from("/dev/input/by-id/device")]).is_err());
}
#[test]
fn routing_contention_retries_only_while_physical_key_remains_down() {
    for release in [false, true] {
        let (mut listener, mut writer, dir) = fake_listener();
        let routing = Lock::take(&dir.join("routing.lock")).unwrap();
        send(&mut writer, 1, ASSISTANT as u16, 1);
        assert!(listener.poll().is_empty());
        assert!(listener.pending.is_some());
        if release {
            send(&mut writer, 1, ASSISTANT as u16, 0);
        }
        drop(routing);
        assert_eq!(
            listener.poll(),
            if release { vec![] } else { vec![Edge::Press] }
        );
        drop(listener);
        fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn heartbeat_is_throttled_but_focus_and_eligibility_publish_immediately() {
    let dir = temp();
    let mut registration = Registration::new(dir.clone()).unwrap();
    registration.update(Some(true), true).unwrap();
    let first = fs::read(&registration.path).unwrap();
    registration.update(Some(true), true).unwrap();
    assert_eq!(fs::read(&registration.path).unwrap(), first);
    registration.update(None, false).unwrap();
    assert_ne!(fs::read(&registration.path).unwrap(), first);
    drop(registration);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn malformed_peer_registration_does_not_block_valid_capture() {
    let dir = temp();
    let mut registration = Registration::new(dir.clone()).unwrap();
    registration.update(Some(true), true).unwrap();
    fs::write(dir.join("reg-malformed-peer"), "invalid 0 1\n").unwrap();
    assert!(registration.capture(now().unwrap()).unwrap().is_some());
    drop(registration);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unsafe_directory_is_rejected() {
    let dir = temp();
    fs::create_dir(&dir).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Registration::new(dir.clone()).is_err());
    fs::remove_dir(dir).unwrap();
}

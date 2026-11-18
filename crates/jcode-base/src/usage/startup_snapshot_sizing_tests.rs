use super::*;

fn fixture(accounts: usize) -> Snapshot {
    let account: AccountUsageSnapshot = serde_json::from_value(serde_json::json!({
        "label": "account \"escaped\" \\ 東京\n", "email": null,
        "exhausted": false, "five_hour_ratio": 0.375,
        "seven_day_ratio": 0.8, "additional_windows": []
    }))
    .unwrap();
    Snapshot {
        schema: SCHEMA,
        socket: "/tmp/daemon.sock".into(),
        identity: SocketIdentity {
            device: 1,
            inode: 2,
            changed_secs: 3,
            changed_nanos: 4,
        },
        instance_id: "daemon".into(),
        pid: std::process::id(),
        published_ms: 12345,
        anthropic: vec![account.clone(); accounts],
        openai: vec![account; accounts],
    }
}

#[test]
fn exact_compact_lengths_and_quota_boundaries() {
    for count in [0, 1, 32, 256] {
        let snapshot = fixture(count);
        assert_eq!(
            serialized_byte_len(&snapshot).unwrap(),
            serde_json::to_vec(&snapshot).unwrap().len() as u64
        );
    }
    let mut snapshot = fixture(0);
    let base = serde_json::to_vec(&snapshot).unwrap().len();
    for target in [MAX_BYTES - 1, MAX_BYTES, MAX_BYTES + 1] {
        snapshot.instance_id = "x".repeat(target as usize - base + "daemon".len());
        assert_eq!(serialized_byte_len(&snapshot).unwrap(), target);
        assert_eq!(serde_json::to_vec(&snapshot).unwrap().len() as u64, target);
    }
}

#[test]
fn serialization_errors_are_not_swallowed() {
    struct Fails;
    impl Serialize for Fails {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional sizing failure"))
        }
    }
    assert_eq!(
        serialized_byte_len(&Fails).unwrap_err().to_string(),
        serde_json::to_vec(&Fails).unwrap_err().to_string()
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut snapshot = fixture(0);
        snapshot.socket = std::ffi::OsString::from_vec(vec![0xff]).into();
        assert_eq!(
            serialized_byte_len(&snapshot).unwrap_err().to_string(),
            serde_json::to_vec(&snapshot).unwrap_err().to_string()
        );
    }
}

#[test]
#[ignore = "release-only CPU comparison probe"]
fn release_sizing_probe() {
    use std::{hint::black_box, time::Instant};
    for accounts in [1, 128] {
        let snapshot = fixture(accounts);
        let iterations = if accounts == 1 { 200_000 } else { 5_000 };
        let mut old = Vec::new();
        let mut new = Vec::new();
        for round in 0..8 {
            for count_only in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let start = Instant::now();
                for _ in 0..iterations {
                    let len = if count_only {
                        serialized_byte_len(black_box(&snapshot)).unwrap()
                    } else {
                        serde_json::to_vec(black_box(&snapshot)).unwrap().len() as u64
                    };
                    black_box(len);
                }
                let elapsed = start.elapsed().as_nanos() as f64 / iterations as f64;
                if count_only {
                    new.push(elapsed);
                } else {
                    old.push(elapsed);
                }
            }
        }
        old.sort_by(f64::total_cmp);
        new.sort_by(f64::total_cmp);
        eprintln!(
            "accounts/provider={accounts} bytes={} old_ns={:.1} count_ns={:.1} ratio={:.3}",
            serialized_byte_len(&snapshot).unwrap(),
            old[4],
            new[4],
            new[4] / old[4]
        );
        assert!(new[4] <= old[4], "reject measured CPU regression");
    }
}

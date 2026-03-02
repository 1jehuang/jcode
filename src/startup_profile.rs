use std::sync::Mutex;
use std::time::Instant;

static PROFILE: Mutex<Option<StartupProfile>> = Mutex::new(None);

pub struct StartupProfile {
    start: Instant,
    marks: Vec<(String, Instant)>,
}

impl StartupProfile {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            marks: vec![("process_start".to_string(), now)],
        }
    }
}

pub fn init() {
    let mut guard = PROFILE.lock().unwrap();
    *guard = Some(StartupProfile::new());
}

pub fn mark(name: &str) {
    if let Ok(mut guard) = PROFILE.lock() {
        if let Some(ref mut profile) = *guard {
            profile.marks.push((name.to_string(), Instant::now()));
        }
    }
}

pub fn report() -> String {
    let guard = PROFILE.lock().unwrap();
    let profile = match guard.as_ref() {
        Some(p) => p,
        None => return "No startup profile recorded".to_string(),
    };

    let total = profile.marks.last().unwrap().1.duration_since(profile.start);
    let mut lines = vec![
        format!("=== Startup Profile ({:.1}ms total) ===", total.as_secs_f64() * 1000.0),
    ];

    for i in 1..profile.marks.len() {
        let delta = profile.marks[i].1.duration_since(profile.marks[i - 1].1);
        let from_start = profile.marks[i].1.duration_since(profile.start);
        let pct = if total.as_nanos() > 0 {
            (delta.as_nanos() as f64 / total.as_nanos() as f64) * 100.0
        } else {
            0.0
        };
        let bar = "█".repeat((pct / 2.0).ceil() as usize);
        lines.push(format!(
            "  {:>7.1}ms  {:>7.1}ms  {:>5.1}%  {:<30} {}",
            from_start.as_secs_f64() * 1000.0,
            delta.as_secs_f64() * 1000.0,
            pct,
            profile.marks[i].0,
            bar,
        ));
    }

    lines.join("\n")
}

pub fn report_to_log() {
    let report = report();
    for line in report.lines() {
        crate::logging::info(line);
    }
}

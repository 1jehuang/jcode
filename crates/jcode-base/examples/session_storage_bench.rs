//! Compare real session workloads through the public storage boundary.
//! Run only against a private copy: source data is never modified by this program.
use anyhow::{Context, Result, bail, ensure};
use jcode_base::session::{Session, SessionBackend, SessionListOptions, SessionStore};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Serialize)]
struct Measurement {
    operation: String,
    backend: String,
    samples: usize,
    median_ms: f64,
    p95_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

fn record(results: &mut Vec<Measurement>, operation: &str, backend: &str, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    let n = samples.len();
    results.push(Measurement {
        operation: operation.into(),
        backend: backend.into(),
        samples: n,
        median_ms: samples[n / 2],
        p95_ms: samples[((n * 95).div_ceil(100)).saturating_sub(1)],
        min_ms: samples[0],
        max_ms: samples[n - 1],
    });
}

fn timed<T>(operation: impl FnOnce() -> Result<T>) -> Result<(T, f64)> {
    let start = Instant::now();
    let value = operation()?;
    Ok((value, start.elapsed().as_secs_f64() * 1000.0))
}

fn normalized(session: &Session, ignore_update_time: bool) -> Result<Value> {
    let mut value = serde_json::to_value(session)?;
    if ignore_update_time {
        value
            .as_object_mut()
            .context("session is not an object")?
            .remove("updated_at");
    }
    Ok(value)
}

fn assert_same(left: &Session, right: &Session, ignore_update_time: bool) -> Result<()> {
    // Deliberately do not print transcripts, ids, or either value on failure.
    let left = normalized(left, ignore_update_time)?;
    let right = normalized(right, ignore_update_time)?;
    if left != right {
        let keys: Vec<_> = left
            .as_object()
            .context("session is not an object")?
            .keys()
            .chain(
                right
                    .as_object()
                    .context("session is not an object")?
                    .keys(),
            )
            .filter(|key| left.get(*key) != right.get(*key))
            .collect();
        bail!("session read-back mismatch in fields {keys:?} (private values withheld)");
    }
    Ok(())
}

fn footprint(root: &Path) -> Result<Value> {
    let mut logical = 0u64;
    let mut allocated = 0u64;
    let mut files = 0u64;
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let meta = entry.metadata()?;
            if meta.is_dir() {
                pending.push(entry.path());
            } else if meta.is_file() {
                files += 1;
                logical += meta.len();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    allocated += meta.blocks() * 512;
                }
                #[cfg(not(unix))]
                {
                    allocated += meta.len();
                }
            }
        }
    }
    Ok(json!({"logical_bytes": logical, "allocated_bytes": allocated, "files": files}))
}

fn copy_legacy(source: &Path, destination: &Path, ids: &[String]) -> Result<()> {
    std::fs::create_dir_all(destination.join("sessions"))?;
    for id in ids {
        for suffix in ["json", "journal.jsonl"] {
            let name = format!("{id}.{suffix}");
            let path = source.join("sessions").join(&name);
            if path.exists() {
                let destination = destination.join("sessions").join(name);
                std::fs::copy(&path, &destination)?;
                let metadata = path.metadata()?;
                let mut times = std::fs::FileTimes::new().set_modified(metadata.modified()?);
                if let Ok(accessed) = metadata.accessed() {
                    times = times.set_accessed(accessed);
                }
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(destination)?
                    .set_times(times)?;
            }
        }
    }
    Ok(())
}

fn samples_by_size(paths: &[(String, PathBuf, u64)]) -> Vec<String> {
    let mut ordered: Vec<_> = paths.iter().collect();
    ordered.sort_by_key(|(_, _, len)| *len);
    let mut result = Vec::new();
    for quantile in 0..16 {
        let index = quantile * (ordered.len() - 1) / 15;
        let id = &ordered[index].0;
        if result.last() != Some(id) {
            result.push(id.clone());
        }
    }
    result
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(
        args.next()
            .context("usage: session_storage_bench SOURCE_ROOT NEW_OUTPUT_ROOT [REPEATS]")?,
    );
    if source == Path::new("--check") {
        let root = PathBuf::from(args.next().context("missing existing output root")?);
        let files = SessionStore::open(root.join("files"), SessionBackend::Files)?;
        let sql = SessionStore::open(root.join("sqlite"), SessionBackend::Sqlite)?;
        for summary in files.list()? {
            assert_same(&files.load(&summary.id)?, &sql.load(&summary.id)?, true)?;
        }
        println!("existing stores agree");
        return Ok(());
    }
    let output = PathBuf::from(args.next().context("missing new output root")?);
    let repeats: usize = args
        .next()
        .map(|n| n.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(7);
    ensure!(repeats >= 3, "use at least three repetitions");
    ensure!(args.next().is_none(), "unexpected argument");
    ensure!(
        !output.exists(),
        "output root must not exist; existing data will not be overwritten"
    );
    ensure!(
        source.join("sessions").is_dir(),
        "source root must contain sessions/"
    );
    std::fs::create_dir_all(&output)?;
    let file_root = output.join("files");
    let sql_root = output.join("sqlite");
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(source.join("sessions"))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") && entry.file_type()?.is_file() {
            let id = path
                .file_stem()
                .context("missing id")?
                .to_string_lossy()
                .into_owned();
            paths.push((id, path, entry.metadata()?.len()));
        }
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    ensure!(!paths.is_empty(), "empty source corpus");
    let input_count = paths.len();
    let mut valid = Vec::new();
    let mut import_failures = 0usize;
    let sql = SessionStore::open(&sql_root, SessionBackend::Sqlite)?;
    let import_start = Instant::now();
    for (index, (id, path, len)) in paths.into_iter().enumerate() {
        // Preserve original timestamps and all persistence fields during import.
        match sql.import_from_path(&path) {
            Ok(()) => valid.push((id, path, len)),
            Err(_) => import_failures += 1,
        }
        if (index + 1) % 250 == 0 {
            eprintln!(
                "imported {} of {} sessions; {} rejected",
                valid.len(),
                input_count,
                import_failures
            );
        }
    }
    let import_ms = import_start.elapsed().as_secs_f64() * 1000.0;
    ensure!(!valid.is_empty(), "no sessions imported");
    let ids: Vec<_> = valid.iter().map(|(id, _, _)| id.clone()).collect();
    copy_legacy(&source, &file_root, &ids)?;
    let files = SessionStore::open(&file_root, SessionBackend::Files)?;
    let validation_start = Instant::now();
    eprintln!("validating {} imported sessions", ids.len());
    for id in &ids {
        assert_same(&files.load(id)?, &sql.load(id)?, false)?;
    }
    let validation_ms = validation_start.elapsed().as_secs_f64() * 1000.0;
    let initial_footprint =
        json!({"files": footprint(&file_root)?, "sqlite": footprint(&sql_root)?});
    let options = SessionListOptions {
        limit: 400,
        ..Default::default()
    };
    // This is a contract check, not a timing of the existing picker parser.
    ensure!(
        serde_json::to_value(files.list_summaries(&options)?)?
            == serde_json::to_value(sql.list_summaries(&options)?)?,
        "summary parity mismatch (private values withheld)"
    );
    let sample_ids = samples_by_size(&valid);
    // Preserve an untouched corpus for the separate picker comparison.
    drop(sql);
    let picker_root = output.join("picker");
    copy_legacy(&source, &picker_root.join("files"), &ids)?;
    let picker_sql = picker_root.join("sqlite");
    std::fs::create_dir_all(&picker_sql)?;
    for entry in std::fs::read_dir(&sql_root)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), picker_sql.join(entry.file_name()))?;
        }
    }
    let sql = SessionStore::open(&sql_root, SessionBackend::Sqlite)?;
    let mut results = Vec::new();
    eprintln!("measuring metadata listing, full loads, and commits");
    let mut file_load = Vec::new();
    let mut sql_load = Vec::new();
    for repetition in 0..repeats {
        for id in &sample_ids {
            if repetition % 2 == 0 {
                file_load.push(timed(|| files.load(id))?.1);
                sql_load.push(timed(|| sql.load(id))?.1);
            } else {
                sql_load.push(timed(|| sql.load(id))?.1);
                file_load.push(timed(|| files.load(id))?.1);
            }
        }
    }
    record(&mut results, "load_size_quantiles", "files", file_load);
    record(&mut results, "load_size_quantiles", "sqlite", sql_load);
    // Exclude legacy retention cleanup from writes, not original full-load timings.
    for id in &sample_ids {
        let mut left = files.load(id)?;
        let mut right = sql.load(id)?;
        left.replace_messages(left.messages.clone());
        files.save_prepared(&mut left)?;
        right.env_snapshots = left.env_snapshots.clone();
        sql.save_prepared(&mut right)?;
        assert_same(&left, &right, true)?;
    }
    for operation in [
        "metadata_commit",
        "title_edit_commit",
        "append_commit",
        "truncate_commit",
    ] {
        let mut file_times = Vec::new();
        let mut sql_times = Vec::new();
        for (index, id) in sample_ids.iter().enumerate() {
            let mut left = files.load(id)?;
            let mut right = sql.load(id)?;
            let message = left
                .messages
                .iter()
                .rev()
                .find(|message| !message.content.is_empty())
                .cloned();
            for repetition in 0..repeats {
                match operation {
                    "metadata_commit" => {
                        let active_at = Some(chrono::Utc::now());
                        left.last_active_at = active_at;
                        right.last_active_at = active_at;
                    }
                    "title_edit_commit" => {
                        let title = Some(format!("storage benchmark {repetition}"));
                        left.custom_title = title.clone();
                        right.custom_title = title;
                    }
                    "append_commit" => {
                        let Some(mut message) = message.clone() else {
                            continue;
                        };
                        message.id = format!("storage_benchmark_{index}_{repetition}");
                        left.append_stored_message(message.clone());
                        right.append_stored_message(message);
                    }
                    "truncate_commit" => {
                        let len = left.messages.len().saturating_sub(1);
                        left.truncate_messages(len);
                        right.truncate_messages(len);
                    }
                    _ => unreachable!(),
                }
                if repetition % 2 == 0 {
                    file_times.push(timed(|| files.save_prepared(&mut left))?.1);
                    sql_times.push(timed(|| sql.save_prepared(&mut right))?.1);
                } else {
                    sql_times.push(timed(|| sql.save_prepared(&mut right))?.1);
                    file_times.push(timed(|| files.save_prepared(&mut left))?.1);
                }
                // Fresh connections verify durable state rather than just the in-memory objects.
                let re_file = SessionStore::open(&file_root, SessionBackend::Files)?.load(id)?;
                let re_sql = SessionStore::open(&sql_root, SessionBackend::Sqlite)?.load(id)?;
                assert_same(&re_file, &re_sql, true).with_context(|| {
                    format!("{operation} sample {index} repetition {repetition}: backend parity")
                })?;
                assert_same(&left, &re_file, false).with_context(|| {
                    format!("{operation} sample {index} repetition {repetition}: files reopen")
                })?;
                assert_same(&right, &re_sql, false).with_context(|| {
                    format!("{operation} sample {index} repetition {repetition}: sqlite reopen")
                })?;
            }
        }
        if file_times.is_empty() {
            bail!("no samples for {operation}");
        }
        record(&mut results, operation, "files", file_times);
        record(&mut results, operation, "sqlite", sql_times);
    }
    let report = json!({
        "format_version": 1,
        "corpus": {"input_sessions": input_count, "imported_sessions": valid.len(), "import_failures": import_failures, "snapshot_bytes": valid.iter().map(|(_, _, len)| len).sum::<u64>()},
        "conditions": {"cache": "warm filesystem; no global cache eviction", "order": "alternating backends", "repetitions": repeats, "load_and_write_size_quantiles": sample_ids.len(), "sqlite_durability": "WAL synchronous=FULL", "file_durability": "existing snapshot/journal backend semantics"},
        "sqlite_import_ms": import_ms,
        "full_corpus_parity_check_ms": validation_ms,
        "initial_footprint": initial_footprint,
        "final_footprint": {"files": footprint(&file_root)?, "sqlite": footprint(&sql_root)?},
        "measurements": results
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

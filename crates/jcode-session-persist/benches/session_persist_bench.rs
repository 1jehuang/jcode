use criterion::{criterion_group, criterion_main, Criterion};
use jcode_session_persist::SessionStore;
use tempfile::tempdir;
use uuid::Uuid;

fn bench_session_store(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    
    c.bench_function("create_session", |b| {
        b.iter(|| {
            let id = Uuid::new_v4().to_string();
            store.create_session(&id, "test_session").unwrap();
        })
    });
}

criterion_group!(benches, bench_session_store);
criterion_main!(benches);
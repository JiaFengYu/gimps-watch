use gimps_watch::{config::Source, parser::parse, reader::Follower, storage::Store};
use serde_json::Value;
use std::{fs, io::Write, path::Path, process::Command};
const NOW: &str = "2026-10-26T12:00:00Z";
fn source(path: &Path) -> Source {
    Source {
        id: "worker".into(),
        kind: "worker_log".into(),
        path: path.into(),
        format: "prime95-text-subset-v1".into(),
        worker_id: None,
    }
}
fn setup() -> (tempfile::TempDir, Store, Follower) {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("worker.log");
    fs::write(&path, "").unwrap();
    let db = Store::open(&d.path().join("state.sqlite")).unwrap();
    let f = Follower::new(&db, "host", source(&path)).unwrap();
    (d, db, f)
}
fn append(path: &Path, s: &str) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(s.as_bytes())
        .unwrap();
}
fn count(db: &Store) -> usize {
    db.events(500, 0).unwrap()["items"]
        .as_array()
        .unwrap()
        .len()
}
#[test]
fn parser_keeps_unknowns_and_rejects_invalid_progress() {
    let p = parse(
        "prime95-text-subset-v1",
        "[Worker #1] Starting PRP test of M1000003 using FFT",
        None,
    );
    assert_eq!(p.assignment_id.as_deref(), Some("exponent:1000003"));
    assert_eq!(p.percent, None);
    assert_eq!(
        parse(
            "prime95-text-subset-v1",
            "[Worker #1] Iteration: 10 / 5 [200%]",
            None
        )
        .kind,
        "diagnostic"
    );
    assert_eq!(
        parse(
            "prime95-text-subset-v1",
            "[Worker #1] Starting PRP test of M5",
            Some("worker-2")
        )
        .kind,
        "diagnostic"
    );
    assert_eq!(
        parse("prime95-text-subset-v1", "unknown record", None).kind,
        "diagnostic"
    );
}
#[test]
fn partial_crlf_and_interleaved_workers() {
    let (_d, mut db, mut f) = setup();
    append(
        &f.source.path,
        "[Worker #1] Starting PRP test of M1000003\r\n[Worker #2] Starting PRP test of M1000033\n[Worker #1] Iteration: 10",
    );
    assert_eq!(f.poll(&mut db, "host", NOW, 65536, false).unwrap(), 2);
    let checkpoint = f.checkpoint.offset;
    assert_eq!(f.poll(&mut db, "host", NOW, 65536, false).unwrap(), 0);
    assert_eq!(f.checkpoint.offset, checkpoint);
    append(&f.source.path, " / 1000003 [0.001%]\n");
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    let workers = db.projections("workers").unwrap();
    assert_eq!(workers[0]["iteration"], 10);
    assert_eq!(workers[1]["iteration"], Value::Null);
    assert_eq!(workers[1]["assignment_id"], "exponent:1000033");
}
#[test]
fn transaction_rollback_and_restart_after_commit() {
    let (d, mut db, mut f) = setup();
    append(
        &f.source.path,
        "[Worker #1] Starting PRP test of M1000003\n",
    );
    assert!(f.poll(&mut db, "host", NOW, 65536, true).is_err());
    assert_eq!(count(&db), 0);
    assert_eq!(db.checkpoint("worker").unwrap().offset, 0);
    assert_eq!(
        db.projections("workers").unwrap().as_array().unwrap().len(),
        0
    );
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    drop(db);
    let mut db = Store::open(&d.path().join("state.sqlite")).unwrap();
    let mut restarted = Follower::new(&db, "host", f.source).unwrap();
    assert_eq!(
        restarted.poll(&mut db, "host", NOW, 65536, false).unwrap(),
        0
    );
    assert_eq!(count(&db), 1);
}
#[test]
fn rotation_drains_old_inode_then_reads_replacement() {
    let (d, mut db, mut f) = setup();
    append(&f.source.path, "[Worker #1] Starting PRP test of M11\n");
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    let old = d.path().join("old.log");
    fs::rename(&f.source.path, &old).unwrap();
    append(&old, "[Worker #1] M11 is not prime.\n");
    fs::write(&f.source.path, "[Worker #1] Starting PRP test of M13\n").unwrap();
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    assert_eq!(count(&db), 3);
    assert_eq!(f.checkpoint.generation, 2);
    assert_eq!(db.projections("workers").unwrap()[0]["exponent"], 13);
}
#[test]
fn detects_truncation_and_same_size_rewrite() {
    let (_d, mut db, mut f) = setup();
    let first = "[Worker #1] Starting PRP test of M11\n";
    append(&f.source.path, first);
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    fs::write(&f.source.path, first.replace("M11", "M13")).unwrap();
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    assert_eq!(f.checkpoint.generation, 2);
    fs::write(&f.source.path, "short\n").unwrap();
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    assert_eq!(f.checkpoint.generation, 3);
    assert_eq!(count(&db), 3);
}
#[test]
fn oversized_and_missing_sources_are_visible() {
    let (_d, mut db, mut f) = setup();
    append(&f.source.path, &"x".repeat(300));
    f.poll(&mut db, "host", NOW, 256, false).unwrap();
    assert_eq!(f.checkpoint.offset, 0);
    assert!(
        db.sources().unwrap()[0]["health"]
            .as_str()
            .unwrap()
            .starts_with("record_too_large")
    );
    fs::remove_file(&f.source.path).unwrap();
    let mut restarted = Follower::new(&db, "host", f.source).unwrap();
    restarted.poll(&mut db, "host", NOW, 256, false).unwrap();
    assert!(
        db.sources().unwrap()[0]["health"]
            .as_str()
            .unwrap()
            .starts_with("unavailable")
    );
}
#[test]
fn result_ids_deduplicate_without_hiding_source_records() {
    let (d, mut db, _) = setup();
    let path = d.path().join("results.jsonl");
    let raw = include_str!("../fixtures/basic-run/results.jsonl");
    fs::write(&path, raw).unwrap();
    let s = Source {
        id: "results".into(),
        kind: "results".into(),
        path,
        format: "prime95-json-subset-v1".into(),
        worker_id: None,
    };
    let mut f = Follower::new(&db, "host", s).unwrap();
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    assert_eq!(count(&db), 2);
    assert_eq!(
        db.status().unwrap()["completion_records"]["explicit_id_results"],
        1
    );
}
#[test]
fn concurrent_writer_checkpoint_is_rejected() {
    let (_d, mut db, mut f) = setup();
    let mut other = Follower::new(&db, "host", f.source.clone()).unwrap();
    append(&f.source.path, "hello\n");
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    assert!(
        other
            .poll(&mut db, "host", NOW, 65536, false)
            .unwrap_err()
            .to_string()
            .contains("another observer")
    );
    assert_eq!(count(&db), 1);
}
#[test]
fn error_does_not_replace_last_progress() {
    let (_d, mut db, mut f) = setup();
    append(
        &f.source.path,
        "[Worker #1] Starting PRP test of M1000003\n[Worker #1] Iteration: 10 / 1000003 [0.001%]\n[Worker #1] ERROR: hardware failure\n",
    );
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    let w = db.projections("workers").unwrap();
    assert_eq!(w[0]["state"], "progress_observed");
    assert_eq!(w[0]["iteration"], 10);
    assert!(
        w[0]["last_error"]["message"]
            .as_str()
            .unwrap()
            .contains("hardware")
    );
}
#[test]
fn cli_replay_is_idempotent_across_processes() {
    let d = tempfile::tempdir().unwrap();
    let db = d.path().join("demo.sqlite");
    for expected in [9, 0] {
        let output = Command::new(env!("CARGO_BIN_EXE_gimps-watch"))
            .args([
                "replay",
                "--manifest",
                "fixtures/basic-run/manifest.json",
                "--db",
            ])
            .arg(&db)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ingested_records"], expected);
    }
}
#[tokio::test]
async fn api_pagination_and_validation() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let (d, mut db, mut f) = setup();
    append(&f.source.path, "one\ntwo\nthree\n");
    f.poll(&mut db, "host", NOW, 65536, false).unwrap();
    let app = gimps_watch::api::router(d.path().join("state.sqlite"));
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/events?limit=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value =
        serde_json::from_slice(&to_bytes(r.into_body(), 100000).await.unwrap()).unwrap();
    assert_eq!(body["next_cursor"], 2);
    let r = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/events?limit=2&cursor=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body: Value =
        serde_json::from_slice(&to_bytes(r.into_body(), 100000).await.unwrap()).unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["next_cursor"], Value::Null);
    for path in [
        "/v1/events?limit=0",
        "/v1/events?cursor=-1",
        "/v1/events?limit=no",
    ] {
        let r = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
    }
}

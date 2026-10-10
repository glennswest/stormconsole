use super::*;
use chrono::TimeZone;

/// PID 1's summary as stormpump 272470b writes it: its own probe of the
/// engine, a container's healthy fastetcd, a container whose stormd went
/// quiet, and a unit that is not running.
const SUMMARY: &str = r#"{"updated":"2026-10-10T12:00:00Z","worst":"stalled","apis":[
 {"source":"stormpump","process":"00-stormblock","api":"volumes","url":"http://127.0.0.1:9090/api/v1/volumes?limit=1",
  "state":"stalled","since":"2026-10-10T11:53:48Z","running":true,"last_ms":null,"p50_ms":4,"p99_ms":9,
  "budget_p50_ms":null,"budget_p99_ms":200,"last_error":"no answer within 10 s","last_check":"2026-10-10T11:59:58Z","checks":800},
 {"source":"stormd","container":"fastetcd","process":"fastetcd","api":"readyz","url":"https://127.0.0.1:2379/readyz",
  "state":"healthy","since":"2026-10-10T08:00:00Z","running":null,"last_ms":3,"p50_ms":3,"p99_ms":5,"budget_p50_ms":null,
  "budget_p99_ms":50,"last_error":null,"last_check":"2026-10-10T11:59:55Z","checks":40,"interval_secs":15,"file_age_secs":2,"stale":false},
 {"source":"stormd","container":"apiserver","process":"rustkube-apiserver","api":"namespace-default","url":"https://127.0.0.1:6443/api/v1/namespaces/default",
  "state":"stalled","since":"2026-10-10T11:58:00Z","running":null,"last_ms":12,"p50_ms":12,"p99_ms":40,"budget_p50_ms":null,
  "budget_p99_ms":200,"last_error":"the stormd file has not changed for 120 s","last_check":"2026-10-10T11:58:00Z","checks":9,
  "interval_secs":15,"file_age_secs":120,"stale":true,"reported_state":"healthy"},
 {"source":"stormpump","process":"10-registry","api":"catalog","url":"http://127.0.0.1:5100/v2/_catalog?n=1",
  "state":"down","since":"2026-10-10T10:00:00Z","running":false,"last_ms":null,"p50_ms":null,"p99_ms":null,
  "budget_p50_ms":null,"budget_p99_ms":200,"last_error":"connection refused","last_check":"2026-10-10T10:00:00Z","checks":3},
 {"source":"stormd","container":"kubelet","process":"rustkube-node","api":"pods","url":"https://127.0.0.1:10250/pods",
  "state":"slow","since":"2026-10-10T11:59:00Z","running":null,"last_ms":812,"p50_ms":300,"p99_ms":812,"budget_p50_ms":100,
  "budget_p99_ms":300,"last_error":null,"last_check":"2026-10-10T11:59:59Z","checks":200,"interval_secs":15,"file_age_secs":1,"stale":false}
]}"#;

fn noon() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap()
}

fn snap(apis: Vec<Api>) -> Snapshot {
    let mut s = Snapshot::new("/run/stormpump/health.json");
    s.source = Source::Summary;
    s.apis = apis;
    model::order(&mut s.apis);
    s
}

#[test]
fn the_summary_reads_every_field_from_both_sources() {
    let s = model::parse_summary(SUMMARY).unwrap();
    assert_eq!(s.apis.len(), 5);
    let engine = &s.apis[0];
    assert_eq!((engine.source.as_str(), engine.process.as_str(), engine.api.as_str()), ("stormpump", "00-stormblock", "volumes"));
    assert_eq!(engine.state, ApiState::Stalled);
    assert_eq!(engine.container, None);
    assert_eq!(engine.running, Some(true));
    assert_eq!(engine.budget_p99_ms, Some(200));
    assert_eq!(engine.key(), "pid1/00-stormblock/volumes");
    let etcd = &s.apis[1];
    assert_eq!(etcd.container.as_deref(), Some("fastetcd"));
    assert_eq!(etcd.interval_secs, Some(15));
    assert_eq!(etcd.key(), "fastetcd/fastetcd/readyz");
    let api = &s.apis[2];
    assert!(api.stale);
    assert_eq!(api.reported_state.as_deref(), Some("healthy"));
    assert!(model::parse_summary("{}").is_err());
    assert!(model::parse_summary("nope").is_err());
}

#[test]
fn a_stall_alerts_naming_the_service_the_probe_and_how_long() {
    let s = model::parse_summary(SUMMARY).unwrap();
    let engine = &s.apis[0];
    assert!(engine.alerting());
    assert_eq!(engine.health(), Health::Error);
    assert_eq!(engine.sentence(noon()), "00-stormblock API volumes STALLED for 6m 12s — no answer within 10 s");
    // A stale container file: stalled, and what its stormd last said.
    let api = &s.apis[2];
    assert!(api.alerting());
    assert_eq!(
        api.sentence(noon()),
        "rustkube-apiserver API namespace-default STALLED for 2m 0s — the stormd file has not changed for 120 s \
         (its stormd last said healthy)"
    );
}

#[test]
fn a_unit_that_is_not_running_keeps_its_state_but_does_not_alert() {
    let s = model::parse_summary(SUMMARY).unwrap();
    let reg = &s.apis[3];
    assert_eq!(reg.state, ApiState::Down);
    assert!(!reg.alerting());
    assert_eq!(reg.health(), Health::Idle);
    assert!(reg.sentence(noon()).ends_with("the unit is not running; this is the last state seen"));
}

#[test]
fn slow_says_its_latency_against_its_budget() {
    let s = model::parse_summary(SUMMARY).unwrap();
    let kubelet = &s.apis[4];
    assert_eq!(kubelet.health(), Health::Warn);
    assert!(!kubelet.alerting());
    assert_eq!(
        kubelet.sentence(noon()),
        "rustkube-node API pods slow for 1m 0s: last 812 ms, p50 300 / p99 812 ms, budget p50 100 / p99 300 ms"
    );
}

#[test]
fn the_node_is_its_worst_api_alerts_first() {
    let s = snap(model::parse_summary(SUMMARY).unwrap().apis);
    // Alerting first, the stall before the down, then slow, then healthy;
    // the unit that is not running after everything that alerts.
    let order: Vec<String> = s.apis.iter().map(|a| a.key()).collect();
    assert_eq!(order[0], "pid1/00-stormblock/volumes");
    assert_eq!(order[1], "apiserver/rustkube-apiserver/namespace-default");
    assert_eq!(*order.last().unwrap(), "fastetcd/fastetcd/readyz");
    let (h, line) = s.health(noon());
    assert_eq!(h, Health::Error);
    assert_eq!(line, "00-stormblock API volumes STALLED for 6m 12s — no answer within 10 s (and 1 more)");
}

#[test]
fn the_feed_has_the_node_and_a_row_per_api() {
    let s = snap(model::parse_summary(SUMMARY).unwrap().apis);
    let c = components(&s, noon());
    assert_eq!(c.len(), 6);
    assert_eq!(c[0].id, "health:node");
    assert_eq!(c[0].health, Health::Error);
    let m = |i: usize, l: &str| c[i].metrics.iter().find(|m| m.label == l).map(|m| m.value.clone());
    assert_eq!(m(0, "stalled").as_deref(), Some("2"));
    assert_eq!(m(0, "slow").as_deref(), Some("1"));
    // The registry is down but not running: not counted as an alert.
    assert_eq!(m(0, "down"), None);
    let engine = c.iter().find(|x| x.id == "health:api:pid1/00-stormblock/volumes").unwrap();
    assert_eq!(engine.kind, "api");
    assert_eq!(engine.label, "00-stormblock · volumes");
    assert_eq!(engine.health, Health::Error);
    assert!(engine.detail.contains("STALLED for 6m 12s"));
    assert_eq!(engine.link.as_deref(), Some("#/health?api=pid1/00-stormblock/volumes"));
    let kubelet = c.iter().find(|x| x.id == "health:api:kubelet/rustkube-node/pods").unwrap();
    let pm = kubelet.metrics.iter().find(|m| m.label == "p50 / p99").unwrap();
    assert_eq!(pm.value, "300 / 812 ms");
    assert_eq!(pm.tone.as_deref(), Some("warn"));
    assert_eq!(kubelet.metrics.iter().find(|m| m.label == "budget").unwrap().value, "100 / 300 ms");
}

#[test]
fn all_healthy_and_nothing_declared_are_said_as_such() {
    let s = model::parse_summary(SUMMARY).unwrap();
    let healthy = snap(vec![s.apis[1].clone()]);
    assert_eq!(healthy.health(noon()), (Health::Ok, "1 API, healthy".to_string()));
    let empty = snap(Vec::new());
    assert_eq!(empty.health(noon()).0, Health::Unknown);
    assert!(empty.health(noon()).1.contains("nothing on this node declares one yet"));
    let unread = Snapshot::new("x");
    assert_eq!(unread.health(noon()).0, Health::Unknown);
}

#[test]
fn durations_read_as_a_person_says_them() {
    assert_eq!(model::duration(45), "45s");
    assert_eq!(model::duration(372), "6m 12s");
    assert_eq!(model::duration(7380), "2h 3m");
    assert_eq!(model::duration(3 * 86400 + 4 * 3600 + 5), "3d 4h");
}

#[test]
fn a_container_file_pid1_could_not_read_stands_for_the_container() {
    let v: Value = serde_json::from_str(
        r#"{"source":"stormd","container":"stormcert","process":null,"api":null,"url":null,"state":"down",
            "since":null,"running":null,"last_error":"not stormd's: no \"items\"","stale":false}"#,
    )
    .unwrap();
    let a = Api::from_value(&v, None);
    assert_eq!(a.service(), "stormcert");
    assert_eq!(a.name(), "stormcert (its API health file)");
    assert!(a.alerting());
    assert_eq!(a.sentence(noon()), "stormcert (its API health file) DOWN — not stormd's: no \"items\"");
}

#[tokio::test]
async fn the_summary_file_is_read_and_its_age_watched() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("health.json");
    std::fs::write(&file, SUMMARY).unwrap();
    let p = ApiHealthPlugin::new(file.to_str().unwrap(), "/nonexistent", "127.0.0.1", vec![]);
    p.poll().await;
    let s = p.snapshot().await;
    assert_eq!(s.source, Source::Summary);
    assert_eq!(s.apis.len(), 5);
    assert!(!s.summary_stale);
    assert_eq!(s.apis[0].key(), "pid1/00-stormblock/volumes");

    // Not rewritten for a minute: the node says so, whatever the APIs say.
    let old = SystemTime::now() - Duration::from_secs(60);
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
    p.poll().await;
    let s = p.snapshot().await;
    assert!(s.summary_stale);
    let (h, line) = s.health(Utc::now());
    assert_eq!(h, Health::Error);
    assert!(line.starts_with("PID 1's API health summary has not been rewritten for 1m"), "{line}");
}

/// A stand-in stormd on a free port, serving `body` with `status`.
async fn stormd(status: u16, body: &'static str) -> u16 {
    use axum::http::StatusCode;
    let app = Router::new().route(
        "/api/v1/health/apis",
        get(move || async move { (StatusCode::from_u16(status).unwrap(), body) }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    port
}

#[tokio::test]
async fn without_the_summary_each_stormd_is_asked() {
    let good = stormd(
        200,
        r#"{"items":[{"process":"fastetcd","api":"readyz","url":"https://127.0.0.1:2379/readyz","state":"stalled",
            "since":"2026-10-10T11:59:00Z","last_ms":null,"p50_ms":3,"p99_ms":5,"budget_p50_ms":null,"budget_p99_ms":50,
            "last_error":"no answer within 5 s","last_check":"2026-10-10T11:59:55Z","checks":40,"interval_secs":15}]}"#,
    )
    .await;
    let old = stormd(404, "not found").await;
    let locked = stormd(401, "unauthorized").await;
    // A port with nothing on it is not worth a word.
    let empty = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let p = ApiHealthPlugin::new("/nonexistent/health.json", "/nonexistent", "127.0.0.1", vec![good, old, locked, empty]);
    p.poll().await;
    let s = p.snapshot().await;
    assert_eq!(s.source, Source::Stormd);
    assert!(s.summary_note.starts_with("/nonexistent/health.json: "), "{}", s.summary_note);
    assert_eq!(s.stormds, vec![format!(":{good}")]);
    assert_eq!(s.apis.len(), 1);
    let a = &s.apis[0];
    assert_eq!(a.container.as_deref(), Some(format!("stormd:{good}").as_str()));
    assert_eq!(a.source, "stormd");
    assert!(a.alerting());
    assert_eq!(s.notes.len(), 2, "{:?}", s.notes);
    assert!(s.notes.iter().any(|n| n == &format!("stormd:{old} predates API health (stormd#49)")));
    assert!(s.notes.iter().any(|n| n == &format!("stormd:{locked} wants credentials for its API health")));
}

#[test]
fn history_is_the_newest_changes_narrowed_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let line = |ts: &str, p: &str, a: &str, from: &str, to: &str| {
        format!(
            r#"{{"ts":"{ts}","process":"{p}","api":"{a}","url":"u","from":"{from}","to":"{to}","from_secs":60,"latency_ms":null,"p50_ms":4,"p99_ms":9,"error":"no answer within 10 s"}}"#
        )
    };
    std::fs::write(
        dir.path().join("00-stormblock.jsonl"),
        [
            line("2026-10-10T11:00:00Z", "00-stormblock", "volumes", "unknown", "healthy"),
            line("2026-10-10T11:53:48Z", "00-stormblock", "volumes", "healthy", "stalled"),
            "garbage".to_string(),
        ]
        .join("\n"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("fastetcd.jsonl"),
        line("2026-10-10T11:30:00Z", "fastetcd", "readyz", "healthy", "slow") + "\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not history").unwrap();
    let d = dir.path().to_str().unwrap();

    let all = history::read(d, None, None, 100);
    assert!(all.available);
    assert_eq!(all.changes.len(), 3);
    assert_eq!(all.changes[0].to, "stalled");
    assert_eq!(all.changes[0].error.as_deref(), Some("no answer within 10 s"));
    assert_eq!(all.changes[1].process, "fastetcd");

    let one = history::read(d, Some("00-stormblock"), Some("volumes"), 100);
    assert_eq!(one.changes.len(), 2);
    assert_eq!(history::read(d, None, None, 1).changes.len(), 1);
    assert!(history::read(d, Some("fastetcd"), Some("other"), 100).changes.is_empty());

    let none = history::read("/nonexistent/history/api", None, None, 100);
    assert!(!none.available);
    assert!(none.note.contains("not mounted into the console"));
}

#[test]
fn a_long_history_file_is_read_from_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut text = String::new();
    // Well past the tail, so the first line read is a fragment.
    for i in 0..5000 {
        text.push_str(&format!(
            r#"{{"ts":"2026-10-{:02}T{:02}:{:02}:00Z","process":"p","api":"a","url":"u","from":"healthy","to":"slow","from_secs":1,"latency_ms":900,"p50_ms":1,"p99_ms":900,"error":null}}"#,
            1 + i / 1440,
            i / 60 % 24,
            i % 60
        ));
        text.push('\n');
    }
    std::fs::write(dir.path().join("p.jsonl"), &text).unwrap();
    let h = history::read(dir.path().to_str().unwrap(), None, None, 2000);
    assert!(h.changes.len() > 500 && h.changes.len() < 5000, "{}", h.changes.len());
    // The newest is the file's last line.
    assert_eq!(h.changes[0].ts.to_rfc3339(), "2026-10-04T11:19:00+00:00");
}

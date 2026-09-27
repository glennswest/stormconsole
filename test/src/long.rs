//! `long` (the night window): waves, per the test standard. Each wave makes
//! a batch of Services in the run's namespace — sized from the node's own
//! pod capacity, never assumed — waits for the console to show every one,
//! deletes them and waits for the console to drop every one. What is
//! measured is the trend across waves: how long the console takes to follow
//! the cluster, how long its feed takes to answer and how large it is, and
//! what each wave leaves behind. A wave slower than the first by more than
//! the tolerance, or any residue, is a failure even when every step passed.

use std::time::{Duration, Instant};

use futures_util::future::join_all;
use serde_json::{json, Value};

use crate::report::{Outcome, Report};
use crate::Ctx;

/// A later wave may be this much slower than the first before it counts as
/// a slowdown — and never below the absolute floor, because a first wave of
/// 300 ms is not a baseline a scheduler hiccup should fail against.
const SLOWDOWN: f64 = 3.0;
const FLOOR_MS: u128 = 5_000;
/// Creates and deletes in flight at once. Kept low: rustkube serializes
/// Service creates (~1.5 s each, rustkube#113), and what is measured here is
/// the console following the cluster, not how fast the apiserver takes a
/// burst.
const IN_FLIGHT: usize = 5;

struct Wave {
    n: usize,
    size: usize,
    /// How long the apiserver took to take the wave, and to delete it —
    /// reported beside the console's numbers, not judged here.
    make_ms: u128,
    delete_ms: u128,
    /// From the last create returning to the console showing all of them.
    appear_ms: u128,
    drain_ms: u128,
    feed_ms: u128,
    feed_bytes: usize,
    residue: usize,
}

pub async fn run(ctx: &Ctx, r: &mut Report) {
    let feed = ctx.console.components().await.unwrap_or_default();
    if !crate::console::has(&feed, "plugin:k8s") {
        r.record("waves", Outcome::Skip("the console's kubernetes plugin is off".into()), 0, None);
        return;
    }
    let capacity = ctx.kube.node_pod_capacity(&ctx.env.node).await;
    let size = ctx.env.wave.unwrap_or_else(|| capacity.unwrap_or(110).clamp(10, 1000));
    r.record(
        "wave-size",
        Outcome::Pass(format!(
            "{size} objects a wave ({})",
            match (ctx.env.wave, capacity) {
                (Some(_), _) => "STORMCONSOLE_TEST_WAVE".to_string(),
                (None, Some(c)) => format!("the node's allocatable pods, {c}"),
                (None, None) => "the node's capacity was unreadable; 110".to_string(),
            }
        )),
        0,
        Some(json!({"wave_size": size})),
    );

    let mut waves: Vec<Wave> = Vec::new();
    let mut n = 0;
    loop {
        // Leave room for one more wave like the slowest so far, and cleanup.
        let slowest = waves.iter().map(|w| w.make_ms + w.delete_ms + w.appear_ms + w.drain_ms).max().unwrap_or(0);
        let need = Duration::from_millis((slowest * 2) as u64) + Duration::from_secs(60);
        if !waves.is_empty() && ctx.env.remaining() < need {
            break;
        }
        n += 1;
        // Vary the size, as the standard asks: full, then smaller and larger
        // around it, so a leak that scales with size is not hidden by a
        // constant one.
        let this = match n % 3 {
            0 => size / 2,
            2 => size + size / 2,
            _ => size,
        }
        .max(1);
        let t = Instant::now();
        match wave(ctx, n, this).await {
            Ok(w) => {
                let ok = w.residue == 0;
                r.record(
                    &format!("wave-{n}"),
                    if ok {
                        Outcome::Pass(format!("{} shown in {} ms, dropped in {} ms", w.size, w.appear_ms, w.drain_ms))
                    } else {
                        Outcome::Fail(format!("{} left behind after the drain", w.residue))
                    },
                    t.elapsed().as_millis(),
                    Some(json!({"wave": w.n, "size": w.size, "make_ms": w.make_ms as u64, "delete_ms": w.delete_ms as u64,
                                "appear_ms": w.appear_ms as u64, "drain_ms": w.drain_ms as u64,
                                "feed_ms": w.feed_ms as u64, "feed_bytes": w.feed_bytes, "residue": w.residue})),
                );
                waves.push(w);
            }
            Err(e) => {
                r.record(&format!("wave-{n}"), Outcome::Fail(e), t.elapsed().as_millis(), None);
                break;
            }
        }
        if ctx.env.remaining() < Duration::from_secs(60) {
            break;
        }
    }
    trend(r, &waves);
    r.run("healthy-after", async {
        match ctx.console.get_open("/healthz").await {
            Ok(a) if a.status == 200 => Outcome::Pass(format!("after {} waves", waves.len())),
            Ok(a) => Outcome::Fail(format!("HTTP {}", a.status)),
            Err(e) => Outcome::Fail(e),
        }
    })
    .await;
}

async fn wave(ctx: &Ctx, n: usize, size: usize) -> Result<Wave, String> {
    let names: Vec<String> = (0..size).map(|i| ctx.env.name(&format!("w{n}-{i}"))).collect();
    let ids: Vec<String> = names.iter().map(|m| format!("k8s:svc:{}/{m}", ctx.kube.namespace)).collect();
    let wait = ctx.env.seen_wait + Duration::from_millis(size as u64 * 50);

    let t = Instant::now();
    for chunk in names.chunks(IN_FLIGHT) {
        let made = join_all(chunk.iter().map(|m| {
            let (path, obj) = (ctx.kube.services(), ctx.kube.service(m));
            async move { ctx.kube.create(&path, &obj).await }
        }))
        .await;
        if let Some(Err(e)) = made.into_iter().find(|x| x.is_err()) {
            return Err(format!("making the wave: {e}"));
        }
    }
    let make_ms = t.elapsed().as_millis();
    let t = Instant::now();
    ctx.console
        .until(wait, |f| {
            let have: std::collections::HashSet<&str> = f.iter().filter_map(|c| c["id"].as_str()).collect();
            ids.iter().all(|i| have.contains(i.as_str()))
        })
        .await
        .map_err(|e| format!("the console did not show all {size}: {e}"))?;
    let appear_ms = t.elapsed().as_millis();

    // The feed at its fullest: how long it takes, how big it is.
    let tf = Instant::now();
    let full = ctx.console.get("/api/v1/components").await?;
    let feed_ms = tf.elapsed().as_millis();
    let feed_bytes = full.text.len();

    let t = Instant::now();
    for chunk in names.chunks(IN_FLIGHT) {
        let gone = join_all(chunk.iter().map(|m| {
            let path = format!("{}/{m}", ctx.kube.services());
            async move { ctx.kube.delete(&path).await }
        }))
        .await;
        if let Some(Err(e)) = gone.into_iter().find(|x| x.is_err()) {
            return Err(format!("draining the wave: {e}"));
        }
    }
    let delete_ms = t.elapsed().as_millis();
    let t = Instant::now();
    let drained = ctx
        .console
        .until(wait, |f| !f.iter().any(|c| c["id"].as_str().is_some_and(|i| ids.iter().any(|x| x == i))))
        .await;
    let drain_ms = t.elapsed().as_millis();
    // Residue: what the console still shows, and what the apiserver still
    // holds, of this wave.
    let feed: Vec<Value> = ctx.console.components().await.unwrap_or_default();
    let shown = feed.iter().filter(|c| c["id"].as_str().is_some_and(|i| ids.iter().any(|x| x == i))).count();
    let sel = format!("{}?labelSelector=storm.io%2Ftest-run%3D{}", ctx.kube.services(), ctx.kube.run_id);
    let held = ctx.kube.get(&sel).await?.and_then(|l| l["items"].as_array().map(|a| a.len())).unwrap_or(0);
    if drained.is_err() && shown == 0 && held == 0 {
        return Err("the drain timed out, then cleared".into());
    }
    Ok(Wave { n, size, make_ms, delete_ms, appear_ms, drain_ms, feed_ms, feed_bytes, residue: shown + held })
}

/// The verdict across waves: slower than the first, or anything left.
fn trend(r: &mut Report, waves: &[Wave]) {
    if waves.len() < 2 {
        r.record("trend", Outcome::Skip(format!("{} wave(s): nothing to compare", waves.len())), 0, None);
        return;
    }
    let first = &waves[0];
    // Per object, so waves of different sizes compare.
    let per = |ms: u128, size: usize| ms as f64 / size.max(1) as f64;
    let base = per(first.appear_ms, first.size);
    let worst = waves
        .iter()
        .skip(1)
        .find(|w| w.appear_ms > FLOOR_MS && per(w.appear_ms, w.size) > base * SLOWDOWN);
    let residue: usize = waves.iter().map(|w| w.residue).sum();
    let feed_first = first.feed_ms.max(1) as f64;
    let feed_worst = waves.iter().skip(1).find(|w| w.feed_ms > FLOOR_MS && w.feed_ms as f64 > feed_first * SLOWDOWN);
    let series = json!({
        "appear_ms": waves.iter().map(|w| w.appear_ms as u64).collect::<Vec<_>>(),
        "feed_ms": waves.iter().map(|w| w.feed_ms as u64).collect::<Vec<_>>(),
        "residue": waves.iter().map(|w| w.residue).collect::<Vec<_>>(),
    });
    let outcome = match (worst, feed_worst, residue) {
        (Some(w), _, _) => Outcome::Fail(format!("wave {} followed the cluster {:.1}× slower per object than wave 1", w.n, per(w.appear_ms, w.size) / base)),
        (_, Some(w), _) => Outcome::Fail(format!("wave {}'s feed answered in {} ms, against {} ms at wave 1", w.n, w.feed_ms, first.feed_ms)),
        (_, _, n) if n > 0 => Outcome::Fail(format!("{n} objects left behind across the waves")),
        _ => Outcome::Pass(format!("{} waves: no slowdown beyond {SLOWDOWN}×, nothing left behind", waves.len())),
    };
    r.record("trend", outcome, 0, Some(series));
}

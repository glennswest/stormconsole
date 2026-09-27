//! `short` (< 2 min): the console is up on the node and does its main job —
//! it shows the cluster as it is, and follows it as it changes.

use serde_json::{json, Value};

use crate::console::has;
use crate::report::{Outcome, Report};
use crate::Ctx;

pub async fn run(ctx: &Ctx, r: &mut Report) {
    r.run("healthz", healthz(ctx)).await;
    r.run("readyz", readyz(ctx)).await;
    r.run("version", version(ctx)).await;
    r.run("app", app(ctx)).await;
    r.run("feed", feed(ctx)).await;
    r.run("nav", nav(ctx)).await;
    appears_and_leaves(ctx, r).await;
}

async fn healthz(ctx: &Ctx) -> Outcome {
    match ctx.console.get_open("/healthz").await {
        Ok(a) if a.status == 200 && a.text.trim() == "ok" => Outcome::Pass("ok".into()),
        Ok(a) => Outcome::Fail(format!("HTTP {} {:?}", a.status, a.text.chars().take(80).collect::<String>())),
        Err(e) => Outcome::Fail(e),
    }
}

/// 503 is a valid answer — an upstream in error — so what is checked is that
/// the console gives its plugins' verdicts at all, without a session.
async fn readyz(ctx: &Ctx) -> Outcome {
    match ctx.console.get_open("/readyz").await {
        Ok(a) if (a.status == 200 || a.status == 503) && a.body["plugins"].is_object() => {
            let plugins = a.body["plugins"].as_object().map(|m| {
                m.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("?"))).collect::<Vec<_>>().join(" ")
            });
            Outcome::Pass(format!("{} · {}", a.body["health"].as_str().unwrap_or("?"), plugins.unwrap_or_default()))
        }
        Ok(a) => Outcome::Fail(format!("HTTP {}: {}", a.status, a.text.chars().take(120).collect::<String>())),
        Err(e) => Outcome::Fail(e),
    }
}

async fn version(ctx: &Ctx) -> Outcome {
    match ctx.console.get_open("/api/version").await {
        Ok(a) if a.status == 200 && a.body["console"].is_string() => Outcome::Pass(format!(
            "console {} · release {}",
            a.body["console"].as_str().unwrap_or(""),
            a.body["release"].as_str().unwrap_or("unknown")
        )),
        Ok(a) => Outcome::Fail(format!("HTTP {}: {}", a.status, a.text.chars().take(120).collect::<String>())),
        Err(e) => Outcome::Fail(e),
    }
}

/// The browser app is embedded: the page and the script it names are served.
async fn app(ctx: &Ctx) -> Outcome {
    let page = match ctx.console.get_open("/").await {
        Ok(a) if a.status == 200 && a.content_type.starts_with("text/html") => a.text,
        Ok(a) => return Outcome::Fail(format!("/ answered {} {}", a.status, a.content_type)),
        Err(e) => return Outcome::Fail(e),
    };
    let Some(src) = page.split("src=\"").nth(1).and_then(|s| s.split('"').next()) else {
        return Outcome::Fail("the page names no script".into());
    };
    let path = if src.starts_with('/') { src.to_string() } else { format!("/{src}") };
    match ctx.console.get_open(&path).await {
        Ok(a) if a.status == 200 && a.content_type.contains("javascript") && a.text.len() > 1000 => {
            Outcome::Pass(format!("{path}: {} bytes", a.text.len()))
        }
        Ok(a) => Outcome::Fail(format!("{path} answered {} {} ({} bytes)", a.status, a.content_type, a.text.len())),
        Err(e) => Outcome::Fail(e),
    }
}

async fn feed(ctx: &Ctx) -> Outcome {
    match ctx.console.components().await {
        Ok(list) => {
            let cards: Vec<String> = list
                .iter()
                .filter_map(|c| c["id"].as_str().and_then(|i| i.strip_prefix("plugin:")).map(str::to_string))
                .collect();
            if cards.is_empty() {
                Outcome::Fail("no plugin cards in the feed".into())
            } else {
                Outcome::Pass(format!("{} components; plugins {}", list.len(), cards.join(" ")))
            }
        }
        Err(e) => Outcome::Fail(e),
    }
}

async fn nav(ctx: &Ctx) -> Outcome {
    match ctx.console.get("/api/v1/console/nav").await {
        Ok(a) if a.status == 200 => {
            let secs = a.body.get("sections").unwrap_or(&a.body).as_array().cloned().unwrap_or_default();
            if secs.is_empty() {
                Outcome::Fail("no navigation".into())
            } else {
                Outcome::Pass(secs.iter().filter_map(|s| s["label"].as_str()).collect::<Vec<_>>().join(", "))
            }
        }
        Ok(a) => Outcome::Fail(format!("HTTP {}", a.status)),
        Err(e) => Outcome::Fail(e),
    }
}

/// The main job: something made in the cluster appears in the console, and
/// goes when it goes. Through the apiserver, in the run's namespace.
async fn appears_and_leaves(ctx: &Ctx, r: &mut Report) {
    let feed = ctx.console.components().await.unwrap_or_default();
    if !has(&feed, "plugin:k8s") {
        r.record("service-appears", Outcome::Skip("the console's kubernetes plugin is off".into()), 0, None);
        r.record("service-leaves", Outcome::Skip("the console's kubernetes plugin is off".into()), 0, None);
        return;
    }
    let name = ctx.env.name("svc");
    let id = format!("k8s:svc:{}/{name}", ctx.kube.namespace);
    if let Err(e) = ctx.kube.create(&ctx.kube.services(), &ctx.kube.service(&name)).await {
        r.record("service-appears", Outcome::Infra(e), 0, None);
        return;
    }
    let seen = ctx.console.until(ctx.env.seen_wait, |f| has(f, &id)).await;
    let ok = match &seen {
        Ok(d) => r.record("service-appears", Outcome::Pass(format!("{id} in the feed")), d.as_millis(), Some(json!({"seen_ms": d.as_millis() as u64}))),
        Err(e) => r.record("service-appears", Outcome::Fail(format!("{id}: {e}")), 0, None),
    };
    let _ = ok;
    if let Err(e) = ctx.kube.delete(&format!("{}/{name}", ctx.kube.services())).await {
        r.record("service-leaves", Outcome::Infra(e), 0, None);
        return;
    }
    match ctx.console.until(ctx.env.seen_wait, |f: &[Value]| !has(f, &id)).await {
        Ok(d) => r.record("service-leaves", Outcome::Pass(format!("{id} gone")), d.as_millis(), Some(json!({"gone_ms": d.as_millis() as u64}))),
        Err(e) => r.record("service-leaves", Outcome::Fail(format!("{id} still shown: {e}")), 0, None),
    };
}

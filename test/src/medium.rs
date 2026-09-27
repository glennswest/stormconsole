//! `medium` (< 30 min): the console's features and failure paths, end to
//! end, all inside the run's namespace. Nothing here writes outside it —
//! the refusals checked are ones the console makes before any write.

use std::time::Duration;

use reqwest::Method;
use serde_json::{json, Value};

use crate::console::has;
use crate::kube::RUN_LABEL;
use crate::report::{Outcome, Report};
use crate::Ctx;

pub async fn run(ctx: &Ctx, r: &mut Report) {
    // What the short suite proves, again: medium stands on its own.
    crate::short::run(ctx, r).await;
    r.run("open-surface", open_surface(ctx)).await;
    r.run("plugin-cards", plugin_cards(ctx)).await;
    r.run("creators", creators(ctx)).await;
    let feed = ctx.console.components().await.unwrap_or_default();
    if !has(&feed, "plugin:k8s") {
        for t in ["websocket-push", "apply-into-project", "apply-needs-a-project", "apply-bad-yaml", "object-yaml",
                  "edit", "edit-conflict", "events", "projects", "delete-through-console"] {
            r.record(t, Outcome::Skip("the console's kubernetes plugin is off".into()), 0, None);
        }
    } else {
        r.run("websocket-push", websocket_push(ctx)).await;
        r.run("apply-into-project", apply_into_project(ctx)).await;
        r.run("apply-needs-a-project", apply_needs_a_project(ctx)).await;
        r.run("apply-bad-yaml", apply_bad_yaml(ctx)).await;
        object_edit_delete(ctx, r).await;
        r.run("projects", projects(ctx)).await;
    }
    r.run("vm-refuses-a-system-namespace", vm_system_namespace(ctx, &feed)).await;
    r.run("proxy-stays-on-its-upstream", proxy_guard(ctx, &feed)).await;
    r.run("unknown-plugin-route", unknown_route(ctx)).await;
}

fn body(a: &crate::console::Answer) -> String {
    a.text.chars().take(160).collect()
}

/// What a stranger may reach, and what they may not. With authentication
/// off everything answers, which is reported rather than failed: it is the
/// state stormcos ships the console in.
async fn open_surface(ctx: &Ctx) -> Outcome {
    let mut notes = Vec::new();
    for p in ["/healthz", "/readyz", "/api/version", "/api/summary", "/api/v1/auth/session"] {
        match ctx.console.get_open(p).await {
            Ok(a) if a.status == 401 => return Outcome::Fail(format!("{p} needs a session; it must not")),
            Ok(_) => {}
            Err(e) => return Outcome::Fail(e),
        }
    }
    let feed = match ctx.console.get_open("/api/v1/components").await {
        Ok(a) => a.status,
        Err(e) => return Outcome::Fail(e),
    };
    match (ctx.auth, feed) {
        (true, 401) => notes.push("auth on: the feed needs a session".to_string()),
        (true, s) => return Outcome::Fail(format!("auth is on and the feed answered a stranger {s}")),
        (false, 200) => notes.push("auth off: every request is an administrator (as stormcos ships it)".to_string()),
        (false, s) => return Outcome::Fail(format!("auth is off and the feed answered {s}")),
    }
    if ctx.auth {
        match ctx.console.call(Method::GET, "/api/v1/components", None, false).await {
            Ok(a) if a.status == 401 => {}
            Ok(a) => return Outcome::Fail(format!("no bearer, feed answered {}", a.status)),
            Err(e) => return Outcome::Fail(e),
        }
    }
    Outcome::Pass(notes.join("; "))
}

async fn plugin_cards(ctx: &Ctx) -> Outcome {
    let feed = match ctx.console.components().await {
        Ok(f) => f,
        Err(e) => return Outcome::Fail(e),
    };
    let cards: Vec<&Value> = feed.iter().filter(|c| c["id"].as_str().is_some_and(|i| i.starts_with("plugin:"))).collect();
    let blank: Vec<&str> = cards
        .iter()
        .filter(|c| c["detail"].as_str().unwrap_or("").is_empty())
        .filter_map(|c| c["id"].as_str())
        .collect();
    if !blank.is_empty() {
        return Outcome::Fail(format!("cards with no line: {}", blank.join(" ")));
    }
    Outcome::Pass(
        cards
            .iter()
            .map(|c| format!("{}={}", c["id"].as_str().unwrap_or("").trim_start_matches("plugin:"), c["health"].as_str().unwrap_or("?")))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

async fn creators(ctx: &Ctx) -> Outcome {
    let a = match ctx.console.get("/api/v1/console/creators").await {
        Ok(a) if a.status == 200 => a,
        Ok(a) => return Outcome::Fail(format!("HTTP {}", a.status)),
        Err(e) => return Outcome::Fail(e),
    };
    let list = a.body.get("creators").unwrap_or(&a.body).as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        return Outcome::Fail("no create forms".into());
    }
    let in_project = list.iter().filter(|c| c["project"] == true).count();
    let project_form = list.iter().any(|c| c["id"] == "k8s:project");
    Outcome::Pass(format!("{} creators, {in_project} asking for a project, Project form {}", list.len(), if project_form { "present" } else { "absent" }))
}

/// The socket pushes a change, not only a first snapshot.
async fn websocket_push(ctx: &Ctx) -> Outcome {
    let name = ctx.env.name("ws");
    let id = format!("k8s:svc:{}/{name}", ctx.kube.namespace);
    let watch = ctx.console.ws_until(ctx.env.seen_wait, |f| has(f, &id));
    let make = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        ctx.kube.create(&ctx.kube.services(), &ctx.kube.service(&name)).await
    };
    let (seen, made) = tokio::join!(watch, make);
    let _ = ctx.kube.delete(&format!("{}/{name}", ctx.kube.services())).await;
    if let Err(e) = made {
        return Outcome::Infra(e);
    }
    match seen {
        Ok((frames, d)) if frames > 1 => Outcome::Pass(format!("{id} pushed in frame {frames} after {} ms", d.as_millis())),
        Ok((_, _)) => Outcome::Fail("only the first snapshot held it: the change was not pushed".into()),
        Err(e) => Outcome::Fail(e),
    }
}

fn configmap(ctx: &Ctx, name: &str, with_ns: bool) -> String {
    let ns = if with_ns { format!("\n  namespace: {}", ctx.kube.namespace) } else { String::new() };
    format!(
        "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: {name}{ns}\n  labels:\n    {RUN_LABEL}: \"{}\"\ndata:\n  made-by: stormconsole-test\n",
        ctx.kube.run_id
    )
}

/// Import YAML into the run's namespace as the project the dialog chose.
async fn apply_into_project(ctx: &Ctx) -> Outcome {
    let name = ctx.env.name("cm");
    let path = format!("/api/plugins/k8s/apply?project={}", ctx.kube.namespace);
    match ctx.console.call(Method::POST, &path, Some(("application/yaml", configmap(ctx, &name, false))), true).await {
        Ok(a) if a.status == 201 => {}
        Ok(a) => return Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
        Err(e) => return Outcome::Fail(e),
    }
    match ctx.kube.get(&format!("/api/v1/namespaces/{}/configmaps/{name}", ctx.kube.namespace)).await {
        Ok(Some(o)) if o.pointer("/data/made-by").is_some() => Outcome::Pass(format!("{name} in {}", ctx.kube.namespace)),
        Ok(_) => Outcome::Fail("201, and the apiserver has no such ConfigMap".into()),
        Err(e) => Outcome::Infra(e),
    }
}

/// No namespace in the document and no project chosen: refused, and nothing
/// lands in `default`.
async fn apply_needs_a_project(ctx: &Ctx) -> Outcome {
    let name = ctx.env.name("nons");
    let a = match ctx.console.call(Method::POST, "/api/plugins/k8s/apply", Some(("application/yaml", configmap(ctx, &name, false))), true).await {
        Ok(a) => a,
        Err(e) => return Outcome::Fail(e),
    };
    let refused = a.status != 201 && a.text.contains("no project");
    let in_default = ctx.kube.get(&format!("/api/v1/namespaces/default/configmaps/{name}")).await;
    match (refused, in_default) {
        (_, Ok(Some(_))) => {
            let _ = ctx.kube.delete(&format!("/api/v1/namespaces/default/configmaps/{name}")).await;
            Outcome::Fail("it was created in default".into())
        }
        (true, _) => Outcome::Pass(format!("HTTP {}: refused, nothing in default", a.status)),
        (false, _) => Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
    }
}

async fn apply_bad_yaml(ctx: &Ctx) -> Outcome {
    let path = format!("/api/plugins/k8s/apply?project={}", ctx.kube.namespace);
    match ctx.console.call(Method::POST, &path, Some(("application/yaml", "kind: [unclosed\n".into())), true).await {
        Ok(a) if a.status == 400 => Outcome::Pass(format!("400: {}", a.body["error"].as_str().unwrap_or(""))),
        Ok(a) => Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
        Err(e) => Outcome::Fail(e),
    }
}

/// A Service's YAML through the console, an edit, a stale edit, its events,
/// and deleting it through the console.
async fn object_edit_delete(ctx: &Ctx, r: &mut Report) {
    let name = ctx.env.name("obj");
    let key = format!("{}/{name}", ctx.kube.namespace);
    let id = format!("k8s:svc:{key}");
    let api = format!("{}/{name}", ctx.kube.services());
    if let Err(e) = ctx.kube.create(&ctx.kube.services(), &ctx.kube.service(&name)).await {
        for t in ["object-yaml", "edit", "edit-conflict", "events", "delete-through-console"] {
            r.record(t, Outcome::Infra(e.clone()), 0, None);
        }
        return;
    }
    let _ = ctx.console.until(ctx.env.seen_wait, |f| has(f, &id)).await;

    r.run("object-yaml", async {
        match ctx.console.get(&format!("/api/plugins/k8s/object/svc/{key}")).await {
            Ok(a) if a.status == 200 && a.body["yaml"].as_str().is_some_and(|y| y.contains("kind: Service")) => {
                Outcome::Pass(format!("{} bytes of YAML", a.body["yaml"].as_str().unwrap_or("").len()))
            }
            Ok(a) => Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
            Err(e) => Outcome::Fail(e),
        }
    })
    .await;

    let stale = ctx.kube.get(&api).await.ok().flatten();
    r.run("edit", async {
        let Some(mut obj) = stale.clone() else { return Outcome::Infra("the Service vanished".into()) };
        obj["metadata"]["labels"]["storm.io/edited"] = json!("yes");
        match ctx.console.call(Method::PUT, &format!("/api/plugins/k8s/object/svc/{key}"), Some(("application/yaml", obj.to_string())), true).await {
            Ok(a) if a.status == 200 => match ctx.kube.get(&api).await {
                Ok(Some(o)) if o.pointer("/metadata/labels/storm.io~1edited") == Some(&json!("yes")) => Outcome::Pass("saved and read back".into()),
                Ok(_) => Outcome::Fail("200, and the label is not on the object".into()),
                Err(e) => Outcome::Infra(e),
            },
            Ok(a) => Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
            Err(e) => Outcome::Fail(e),
        }
    })
    .await;
    r.run("edit-conflict", async {
        // The object as it was before the edit: its resourceVersion is stale,
        // and saving it must be refused rather than overwrite the edit.
        let Some(mut obj) = stale.clone() else { return Outcome::Infra("the Service vanished".into()) };
        obj["metadata"]["labels"]["storm.io/stale"] = json!("yes");
        match ctx.console.call(Method::PUT, &format!("/api/plugins/k8s/object/svc/{key}"), Some(("application/yaml", obj.to_string())), true).await {
            Ok(a) if a.status == 409 => Outcome::Pass("409: the stale save was refused".into()),
            Ok(a) => Outcome::Fail(format!("a stale save answered {}: {}", a.status, body(&a))),
            Err(e) => Outcome::Fail(e),
        }
    })
    .await;
    r.run("events", async {
        match ctx.console.get(&format!("/api/v1/console/events?id={id}")).await {
            Ok(a) if a.status == 200 && a.body.is_object() => Outcome::Pass(format!(
                "available={} · {} events",
                a.body["available"],
                a.body["items"].as_array().map(|i| i.len()).unwrap_or(0)
            )),
            Ok(a) => Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
            Err(e) => Outcome::Fail(e),
        }
    })
    .await;
    r.run("delete-through-console", async {
        let path = format!("/api/plugins/k8s/raw/api/v1/namespaces/{}/services/{name}", ctx.kube.namespace);
        match ctx.console.call(Method::DELETE, &path, None, true).await {
            Ok(a) if a.status == 200 => {}
            Ok(a) => return Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
            Err(e) => return Outcome::Fail(e),
        }
        match (ctx.kube.get(&api).await, ctx.console.until(ctx.env.seen_wait, |f| !has(f, &id)).await) {
            (Ok(None), Ok(d)) => Outcome::Pass(format!("gone from the apiserver, and from the feed in {} ms", d.as_millis())),
            (Ok(Some(_)), _) => Outcome::Fail("the apiserver still has it".into()),
            (_, Err(e)) => Outcome::Fail(format!("still in the feed: {e}")),
            (Err(e), _) => Outcome::Infra(e),
        }
    })
    .await;
}

async fn projects(ctx: &Ctx) -> Outcome {
    match ctx.console.get("/api/plugins/k8s/projects").await {
        Ok(a) if a.status == 200 && a.body["projects"].is_array() => Outcome::Pass(format!(
            "served={} · {} projects",
            a.body["served"],
            a.body["projects"].as_array().map(|p| p.len()).unwrap_or(0)
        )),
        Ok(a) => Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
        Err(e) => Outcome::Fail(e),
    }
}

/// A machine is never created in a system namespace — refused before any
/// write, so this makes nothing.
async fn vm_system_namespace(ctx: &Ctx, feed: &[Value]) -> Outcome {
    if !has(feed, "plugin:vm") {
        return Outcome::Skip("the console's VM plugin is off".into());
    }
    let form = json!({"name": ctx.env.name("vm"), "golden": "none", "namespace": "default"});
    match ctx.console.call(Method::POST, "/api/plugins/vm/create", Some(("application/json", form.to_string())), true).await {
        Ok(a) if a.status == 400 && a.text.contains("system namespace") => Outcome::Pass("400: default is a system namespace".into()),
        Ok(a) => Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
        Err(e) => Outcome::Fail(e),
    }
}

/// A plugin's proxy forwards only its upstream's surface.
async fn proxy_guard(ctx: &Ctx, feed: &[Value]) -> Outcome {
    if !has(feed, "plugin:ipmi") {
        return Outcome::Skip("the console's stormipmi plugin is off".into());
    }
    match ctx.console.get("/api/plugins/ipmi/proxy/readyz").await {
        Ok(a) if a.status == 404 => Outcome::Pass("a path outside the Machines API: 404".into()),
        Ok(a) => Outcome::Fail(format!("HTTP {}: {}", a.status, body(&a))),
        Err(e) => Outcome::Fail(e),
    }
}

async fn unknown_route(ctx: &Ctx) -> Outcome {
    match ctx.console.get("/api/plugins/no-such-plugin/x").await {
        Ok(a) if a.status == 404 => Outcome::Pass("404".into()),
        Ok(a) => Outcome::Fail(format!("HTTP {} {}", a.status, a.content_type)),
        Err(e) => Outcome::Fail(e),
    }
}

//! stormconsole's test container (stormcentral `docs/test-standard.md`).
//!
//! The console is reached on the node (`STORM_NODE:9094`); what it should
//! show is made through the apiserver (`STORM_API`) in the run's own
//! namespace, labelled `storm.io/test-run`. No machine is assumed: the only
//! requirement is the console service itself, and a node without it reports
//! every test as a skip, never a pass.

pub mod console;
pub mod env;
pub mod kube;
pub mod long;
pub mod medium;
pub mod report;
pub mod short;

use console::Console;
use env::Env;
use kube::Kube;
use report::{Outcome, Report};

/// Everything a suite needs, checked once.
pub struct Ctx {
    pub env: Env,
    pub console: Console,
    pub kube: Kube,
    /// Whether the console demands a session or bearer.
    pub auth: bool,
}

pub async fn run(suite: Option<String>) -> i32 {
    let env = Env::read(suite);
    let mut r = Report::new();
    let missing = env.missing();
    if !missing.is_empty() {
        r.record("environment", Outcome::Infra(format!("the runner did not set {}", missing.join(", "))), 0, None);
        return r.finish();
    }
    let (console, kube) = match (Console::new(&env), Kube::new(&env)) {
        (Ok(c), Ok(k)) => (c, k),
        (Err(e), _) | (_, Err(e)) => {
            r.record("environment", Outcome::Infra(e), 0, None);
            return r.finish();
        }
    };
    // Is there a console on this node at all? It is started on single-node
    // clusters and optional elsewhere: absent is a skip, not a failure.
    match console.get_open("/healthz").await {
        Ok(a) if a.status == 200 => {}
        Ok(a) => {
            r.record("console-present", Outcome::Fail(format!("{}/healthz answered {}", console.base, a.status)), 0, None);
            return r.finish();
        }
        Err(e) => {
            r.record(
                "console-present",
                Outcome::Skip(format!("no console at {} ({e}) — requires: service stormconsole", console.base)),
                0,
                None,
            );
            return r.finish();
        }
    }
    // With authentication on and no token to present, nothing past the open
    // endpoints can be checked. Said, not passed.
    let session = console.get_open("/api/v1/auth/session").await.map(|a| a.body).unwrap_or_default();
    let auth = session["required"].as_bool().unwrap_or(false);
    if auth && env.console_token.is_none() {
        r.record(
            "console-auth",
            Outcome::Skip("the console requires a session and STORMCONSOLE_TOKEN is not set".into()),
            0,
            None,
        );
        return r.finish();
    }
    let ctx = Ctx { env, console, kube, auth };
    match ctx.env.suite.as_str() {
        "short" => short::run(&ctx, &mut r).await,
        "medium" => medium::run(&ctx, &mut r).await,
        "long" => long::run(&ctx, &mut r).await,
        other => {
            r.record("suite", Outcome::Infra(format!("no suite {other:?}: short, medium or long")), 0, None);
        }
    }
    // Leave the namespace as it was found, whatever happened above.
    if let Err(e) = ctx.kube.sweep().await {
        r.record("cleanup", Outcome::Fail(e), 0, None);
    }
    r.finish()
}

//! `/metrics`: the console's own numbers in Prometheus's text format (#41).
//!
//! Only what the console itself knows about itself: each plugin's health and
//! count of components, how long the feed takes to build, who is being pushed
//! it, and what a plugin adds. What it shows of the cluster is ironprom's, from
//! the components themselves. Every name is the console's own, so nothing here
//! needs escaping.

use std::fmt::Write;

use console_core::{FeedStats, Health};

pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

const HEALTHS: [(Health, &str); 5] = [
    (Health::Ok, "ok"),
    (Health::Warn, "warn"),
    (Health::Error, "error"),
    (Health::Idle, "idle"),
    (Health::Unknown, "unknown"),
];

fn head(out: &mut String, name: &str, help: &str, kind: &str) {
    let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
}

pub fn render(version: &str, s: &FeedStats) -> String {
    let mut out = String::new();
    head(&mut out, "stormconsole_build_info", "This console's version.", "gauge");
    let _ = writeln!(out, "stormconsole_build_info{{version=\"{version}\"}} 1");

    head(&mut out, "stormconsole_plugin_health", "1 for the health each plugin is in, 0 for the others.", "gauge");
    for p in &s.plugins {
        for (health, word) in HEALTHS {
            let _ = writeln!(out, "stormconsole_plugin_health{{plugin=\"{}\",health=\"{word}\"}} {}", p.name, u8::from(p.health == health));
        }
    }
    head(&mut out, "stormconsole_plugin_components", "Components each plugin contributes to the feed.", "gauge");
    for p in &s.plugins {
        let _ = writeln!(out, "stormconsole_plugin_components{{plugin=\"{}\"}} {}", p.name, p.components);
    }

    head(&mut out, "stormconsole_feed_refreshes_total", "Times the feed has been built.", "counter");
    let _ = writeln!(out, "stormconsole_feed_refreshes_total {}", s.refreshes);
    head(&mut out, "stormconsole_feed_refresh_seconds", "How long the last build of the feed took.", "gauge");
    let _ = writeln!(out, "stormconsole_feed_refresh_seconds {}", s.refresh_seconds);
    head(&mut out, "stormconsole_websocket_clients", "Websockets being pushed the feed.", "gauge");
    let _ = writeln!(out, "stormconsole_websocket_clients {}", s.clients);

    for sample in &s.samples {
        head(&mut out, sample.name, sample.help, if sample.counter { "counter" } else { "gauge" });
        let _ = writeln!(out, "{} {}", sample.name, sample.value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use console_core::{PluginStat, Sample};

    #[test]
    fn each_series_has_its_help_and_type_and_one_health_is_set() {
        let text = render(
            "1.2.3",
            &FeedStats {
                refreshes: 7,
                refresh_seconds: 0.25,
                clients: 2,
                plugins: vec![PluginStat { name: "k8s".into(), health: Health::Error, components: 12 }],
                samples: vec![Sample { name: "stormconsole_logs_duplicates_total", help: "Folded.", counter: true, value: 9.0 }],
            },
        );
        for line in [
            "stormconsole_build_info{version=\"1.2.3\"} 1",
            "stormconsole_plugin_health{plugin=\"k8s\",health=\"error\"} 1",
            "stormconsole_plugin_health{plugin=\"k8s\",health=\"ok\"} 0",
            "stormconsole_plugin_components{plugin=\"k8s\"} 12",
            "stormconsole_feed_refreshes_total 7",
            "stormconsole_feed_refresh_seconds 0.25",
            "stormconsole_websocket_clients 2",
            "# TYPE stormconsole_logs_duplicates_total counter",
            "stormconsole_logs_duplicates_total 9",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} missing from:\n{text}");
        }
        assert_eq!(text.matches("health=\"").count(), 5, "every state, so a change reads as one series falling");
        assert_eq!(text.lines().filter(|l| l.contains("stormconsole_plugin_health{") && l.ends_with(" 1")).count(), 1);
        for name in text.lines().filter(|l| !l.starts_with('#')).map(|l| l.split(['{', ' ']).next().unwrap()) {
            assert!(text.contains(&format!("# TYPE {name} ")), "{name} has no TYPE");
        }
    }
}

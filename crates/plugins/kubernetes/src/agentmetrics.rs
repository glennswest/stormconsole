//! The Cilium agent's own counters, on the pod network's card (#4).
//!
//! `prometheus-serve-addr` is `127.0.0.1:9962` on a cilium node (stormcos `50-cilium-config.yaml`), and the
//! console shares the host network, so this node's agent is the one read. Names from Cilium v1.20.2's
//! `Documentation/observability/metrics.rst`:
//!
//! - `cilium_drop_count_total{reason, direction}`: there is no policy-verdict counter at the agent, so what policy
//!   denied is the `reason="Policy denied"` series.
//! - `cilium_forward_count_total{direction}`.
//! - `cilium_bpf_map_pressure{map_name}`, a fraction, published only once a map is past a threshold, so absent
//!   means nothing is near full.
//!
//! Over time, per reason, is the Metrics page's (#46). A card only says whether to go and look.

use std::time::Instant;

use console_core::{ComponentSummary, Health, Metric};

/// A BPF map this full is dropping new connections, or soon will.
const PRESSURE_WARN: f64 = 0.9;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counters {
    pub drops: f64,
    pub denied: f64,
    pub forwards: f64,
    /// The fullest map, and how full.
    pub pressure: Option<(String, f64)>,
}

/// One label's value from `{a="b",c="d"}`.
fn label<'a>(labels: &'a str, key: &str) -> Option<&'a str> {
    let at = labels.find(&format!("{key}=\""))? + key.len() + 2;
    let rest = &labels[at..];
    Some(&rest[..rest.find('"')?])
}

pub fn parse(text: &str) -> Counters {
    let mut c = Counters::default();
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let (series, value) = match line.rfind('}') {
            Some(end) => (&line[..=end], line[end + 1..].trim()),
            None => match line.split_once(char::is_whitespace) {
                Some((n, v)) => (n, v.trim()),
                None => continue,
            },
        };
        let Some(v) = value.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()) else { continue };
        let (name, labels) = series.split_once('{').unwrap_or((series, ""));
        match name {
            "cilium_drop_count_total" => {
                c.drops += v;
                if label(labels, "reason") == Some("Policy denied") {
                    c.denied += v;
                }
            }
            "cilium_forward_count_total" => c.forwards += v,
            "cilium_bpf_map_pressure" if c.pressure.as_ref().is_none_or(|(_, worst)| v > *worst) => {
                c.pressure = Some((label(labels, "map_name").unwrap_or("?").to_string(), v));
            }
            _ => {}
        }
    }
    c
}

fn count(v: f64) -> String {
    match v {
        v if v >= 1e9 => format!("{:.1}G", v / 1e9),
        v if v >= 1e6 => format!("{:.1}M", v / 1e6),
        v if v >= 1e4 => format!("{:.1}k", v / 1e3),
        v => format!("{v:.0}"),
    }
}

/// The last scrape, and the drops a second since the one before it.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    pub counters: Counters,
    pub drops_per_sec: Option<f64>,
    at: Option<Instant>,
}

impl Reading {
    /// The next reading from this one. A counter that went backwards is an agent that restarted, not a rate.
    pub fn next(&self, counters: Counters, now: Instant) -> Reading {
        let drops_per_sec = self.at.and_then(|then| {
            let secs = now.duration_since(then).as_secs_f64();
            (secs > 0.0 && counters.drops >= self.counters.drops).then(|| (counters.drops - self.counters.drops) / secs)
        });
        Reading { counters, drops_per_sec, at: Some(now) }
    }
}

/// Put a reading on the card. Drops are how a network works, so they are coloured and not a fault; a map about
/// to fill is one. Nothing near full means nothing to say about it.
pub fn onto_card(card: &mut ComponentSummary, r: &Reading) {
    let c = &r.counters;
    let mut metrics = vec![
        Metric::new("forwarded", count(c.forwards)).tone("muted"),
        Metric::new(
            "dropped",
            match r.drops_per_sec {
                Some(rate) if rate >= 0.05 => format!("{} ({rate:.1}/s)", count(c.drops)),
                _ => count(c.drops),
            },
        )
        .tone(if r.drops_per_sec.is_some_and(|rate| rate >= 0.05) { "warn" } else { "muted" }),
        Metric::new("denied by policy", count(c.denied)).tone("muted"),
    ];
    if let Some((map, full)) = &c.pressure {
        let hot = *full >= PRESSURE_WARN;
        metrics.push(Metric::new("fullest map", format!("{map} {:.0}%", full * 100.0)).tone(if hot { "warn" } else { "muted" }));
        if hot {
            card.detail = format!("{} · BPF map {map} is {:.0}% full", card.detail, full * 100.0);
            // Whatever the CRDs said, short of something already worse.
            if card.health != Health::Error {
                card.health = Health::Warn;
            }
        }
    }
    card.metrics.extend(metrics);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRAPE: &str = r#"# HELP cilium_drop_count_total Total dropped packets, tagged by drop reason and ingress/egress direction
# TYPE cilium_drop_count_total counter
cilium_drop_count_total{direction="EGRESS",reason="Policy denied"} 12
cilium_drop_count_total{direction="INGRESS",reason="Policy denied"} 30
cilium_drop_count_total{direction="INGRESS",reason="Stale or unroutable IP"} 8
cilium_forward_count_total{direction="EGRESS"} 15000
cilium_forward_count_total{direction="INGRESS"} 5000
cilium_bpf_map_pressure{map_name="cilium_lb4_services_v2"} 0.2
cilium_bpf_map_pressure{map_name="cilium_ct4_global"} 0.93
cilium_endpoint_count 4
"#;

    #[test]
    fn a_scrape_is_read_across_its_labels() {
        let c = parse(SCRAPE);
        assert_eq!(c.drops, 50.0);
        assert_eq!(c.denied, 42.0);
        assert_eq!(c.forwards, 20000.0);
        assert_eq!(c.pressure, Some(("cilium_ct4_global".to_string(), 0.93)));
    }

    #[test]
    fn nothing_near_full_is_no_pressure_at_all() {
        assert_eq!(parse("cilium_forward_count_total{direction=\"EGRESS\"} 1\n").pressure, None);
    }

    #[test]
    fn a_rate_needs_two_readings_and_a_restart_is_not_one() {
        let t0 = Instant::now();
        let first = Reading::default().next(Counters { drops: 100.0, ..Default::default() }, t0);
        assert_eq!(first.drops_per_sec, None);
        let t1 = t0 + std::time::Duration::from_secs(10);
        let second = first.next(Counters { drops: 150.0, ..Default::default() }, t1);
        assert_eq!(second.drops_per_sec, Some(5.0));
        let t2 = t1 + std::time::Duration::from_secs(10);
        let restarted = second.next(Counters { drops: 3.0, ..Default::default() }, t2);
        assert_eq!(restarted.drops_per_sec, None);
    }

    fn card() -> ComponentSummary {
        ComponentSummary {
            id: "k8s:cilium".into(),
            kind: "cni".into(),
            label: "Pod network".into(),
            health: Health::Ok,
            detail: "4/4 endpoints ready".into(),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        }
    }

    #[test]
    fn a_map_about_to_fill_is_a_warning_and_drops_are_not() {
        let mut quiet = card();
        let counters = Counters { drops: 500.0, denied: 500.0, forwards: 1.0, pressure: Some(("cilium_lb4".into(), 0.4)) };
        onto_card(&mut quiet, &Reading { counters, drops_per_sec: Some(9.0), at: None });
        assert_eq!(quiet.health, Health::Ok, "drops are how a network works");
        assert!(quiet.metrics.iter().any(|m| m.label == "dropped" && m.value == "500 (9.0/s)"));

        let mut hot = card();
        onto_card(&mut hot, &Reading { counters: parse(SCRAPE), drops_per_sec: None, at: None });
        assert_eq!(hot.health, Health::Warn);
        assert!(hot.detail.contains("cilium_ct4_global is 93% full"), "{}", hot.detail);

        // A card with nothing to report but this says this.
        let mut idle = card();
        idle.health = Health::Idle;
        onto_card(&mut idle, &Reading { counters: parse(SCRAPE), drops_per_sec: None, at: None });
        assert_eq!(idle.health, Health::Warn);

        // A card already an error stays one.
        let mut down = card();
        down.health = Health::Error;
        onto_card(&mut down, &Reading { counters: parse(SCRAPE), drops_per_sec: None, at: None });
        assert_eq!(down.health, Health::Error);
    }

    #[test]
    fn counts_read_at_a_glance() {
        assert_eq!(count(42.0), "42");
        assert_eq!(count(12_500.0), "12.5k");
        assert_eq!(count(3_400_000.0), "3.4M");
        assert_eq!(count(2_000_000_000.0), "2.0G");
    }
}

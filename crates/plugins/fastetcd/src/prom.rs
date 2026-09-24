//! Prometheus text exposition, read just far enough for a status card.
//!
//! fastetcd's `/metrics` is unlabelled gauges and counters with etcd's own
//! names, so a sample is a name and a number. Labelled samples are kept
//! under their bare name as well as their full one, so `x{a="b"} 1` answers
//! to `x` when it is the only one — the case every metric here is in.

use std::collections::BTreeMap;

#[derive(Debug, Default, Clone)]
pub struct Samples(BTreeMap<String, f64>);

impl Samples {
    pub fn parse(text: &str) -> Self {
        let mut out = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // name{labels} value [timestamp]
            let (series, rest) = match line.find('}') {
                Some(end) => (&line[..=end], line[end + 1..].trim()),
                None => match line.split_once(char::is_whitespace) {
                    Some((n, r)) => (n, r.trim()),
                    None => continue,
                },
            };
            let Some(value) = rest.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()) else {
                continue;
            };
            let bare = series.split('{').next().unwrap_or(series);
            // prometheus_client suffixes counters with `_total` in the
            // exposition whatever they were registered as; nothing to undo.
            out.entry(bare.to_string()).or_insert(value);
            out.insert(series.to_string(), value);
        }
        Self(out)
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        self.0.get(name).copied()
    }

    pub fn u64(&self, name: &str) -> Option<u64> {
        self.get(name).filter(|v| *v >= 0.0).map(|v| v as u64)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real scrape of fastetcd's /metrics, trimmed.
    const SCRAPE: &str = "\
# HELP etcd_server_has_leader Whether this node has a known leader (1) or not (0).
# TYPE etcd_server_has_leader gauge
etcd_server_has_leader 1
# HELP etcd_server_leader_changes_seen_total Total number of times the locally-observed leader has changed.
# TYPE etcd_server_leader_changes_seen_total counter
etcd_server_leader_changes_seen_total_total 1
etcd_mvcc_db_total_size_in_bytes 1654784
etcd_server_quota_backend_bytes 2147483648
fastetcd_store_space_used_ratio 0.0007705688476562
etcd_debugging_mvcc_current_revision 412
grpc_server_handled_total{grpc_method=\"Range\",grpc_code=\"OK\"} 17 1690000000000
# EOF
";

    #[test]
    fn names_values_and_labels() {
        let s = Samples::parse(SCRAPE);
        assert_eq!(s.u64("etcd_server_has_leader"), Some(1));
        assert_eq!(s.u64("etcd_mvcc_db_total_size_in_bytes"), Some(1654784));
        assert_eq!(s.u64("etcd_debugging_mvcc_current_revision"), Some(412));
        assert!((s.get("fastetcd_store_space_used_ratio").unwrap() - 0.00077).abs() < 1e-5);
        assert_eq!(s.u64("grpc_server_handled_total"), Some(17));
        assert_eq!(
            s.u64("grpc_server_handled_total{grpc_method=\"Range\",grpc_code=\"OK\"}"),
            Some(17)
        );
        assert_eq!(s.get("absent"), None);
    }

    #[test]
    fn garbage_is_not_a_metric() {
        let s = Samples::parse("<html>not found</html>\nnot found\n");
        assert!(s.is_empty());
    }
}

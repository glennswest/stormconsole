//! stormdrive's `storage.storm.io/v1` objects for the Storage page (#96), reduced. A `Drive` carries its
//! whole history and format plan; the page wants a line per drive, and a rack has 1,600.
//! Shapes: stormdrive `src/api/kube.rs`.

use serde_json::{json, Value};

fn at<'a>(o: &'a Value, ptr: &str) -> &'a Value {
    o.pointer(ptr).unwrap_or(&Value::Null)
}

pub fn drive(o: &Value, node: &str) -> Value {
    json!({
        "node": node,
        "name": at(o, "/status/name"),
        "path": at(o, "/status/path"),
        "serial": at(o, "/status/serial"),
        "model": at(o, "/status/model"),
        "kind": at(o, "/status/kind"),
        "capacityBytes": at(o, "/status/capacityBytes"),
        "enclosure": at(o, "/status/enclosure"),
        "bay": at(o, "/status/bay"),
        "membership": at(o, "/status/membership"),
        "designation": at(o, "/spec/designation"),
        "activity": at(o, "/status/activity"),
        "health": at(o, "/status/health/status"),
        "temperatureC": at(o, "/status/health/temperature_c"),
        "wearPct": at(o, "/status/health/wear_pct"),
    })
}

pub fn enclosure(o: &Value, node: &str) -> Value {
    json!({
        "node": node,
        "name": at(o, "/metadata/name"),
        "shelf": at(o, "/status/shelf"),
        "hbas": at(o, "/status/hbas"),
        "driveCount": at(o, "/status/driveCount"),
        "worstHealth": at(o, "/status/worstHealth"),
        "needsReformat": at(o, "/status/needsReformat"),
        "status": at(o, "/status/enclosure/status"),
        "paths": at(o, "/status/enclosure/paths"),
        "maxTemperatureC": at(o, "/status/enclosure/maxTemperatureC"),
        "powerSupplies": at(o, "/status/enclosure/powerSupplies"),
        "fans": at(o, "/status/enclosure/fans"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drive_is_a_line_and_its_history_stays_behind() {
        let o = json!({"spec": {"designation": "none"}, "status": {"name": "sda", "serial": "Z1", "capacityBytes": 10,
            "bay": 3, "enclosure": "shelf0", "health": {"status": "healthy", "temperature_c": 31, "wear_pct": 4},
            "history": [{"a": 1}], "plannedFormat": {"blockSize": 4096}}});
        let d = drive(&o, "n1");
        assert_eq!((d["node"].as_str(), d["serial"].as_str(), d["bay"].as_i64()), (Some("n1"), Some("Z1"), Some(3)));
        assert_eq!((d["health"].as_str(), d["temperatureC"].as_i64(), d["wearPct"].as_i64()), (Some("healthy"), Some(31), Some(4)));
        assert!(!d.to_string().contains("history") && !d.to_string().contains("plannedFormat"));
    }

    #[test]
    fn an_enclosure_with_nothing_read_from_it_is_nulls() {
        let o = json!({"metadata": {"name": "e1"}, "status": {"driveCount": 2, "enclosure": null, "drives": [{"x": 1}]}});
        let e = enclosure(&o, "n1");
        assert_eq!((e["name"].as_str(), e["driveCount"].as_i64()), (Some("e1"), Some(2)));
        assert!(e["powerSupplies"].is_null());
        assert!(e.get("drives").is_none());
    }
}

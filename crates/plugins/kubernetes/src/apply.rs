//! YAML in, resources out. The console's "Import YAML" and every per-kind
//! "+ Create" post here; the documents are turned into JSON and posted to
//! the collection their apiVersion/kind/namespace names.

use console_core::Creator;
use serde_json::Value;

/// Split a YAML stream into JSON objects. Empty documents are skipped.
pub fn parse_documents(text: &str) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(text) {
        let v: serde_yaml::Value = serde::Deserialize::deserialize(doc).map_err(|e| format!("yaml: {e}"))?;
        if v.is_null() {
            continue;
        }
        let j = serde_json::to_value(v).map_err(|e| format!("yaml → json: {e}"))?;
        if !j.is_object() {
            return Err("a document is not a mapping".into());
        }
        out.push(j);
    }
    Ok(out)
}

const CLUSTER_SCOPED: &[&str] = &[
    "Namespace",
    "Node",
    "PersistentVolume",
    "ClusterRole",
    "ClusterRoleBinding",
    "StorageClass",
    "CustomResourceDefinition",
    "IngressClass",
    "PriorityClass",
    "CSIDriver",
    "CSINode",
    "RuntimeClass",
    "APIService",
];

fn plural(kind: &str) -> String {
    match kind {
        "Endpoints" => "endpoints".into(),
        "Ingress" => "ingresses".into(),
        "NetworkPolicy" => "networkpolicies".into(),
        "StorageClass" => "storageclasses".into(),
        "IngressClass" => "ingressclasses".into(),
        "PriorityClass" => "priorityclasses".into(),
        "RuntimeClass" => "runtimeclasses".into(),
        "PodDisruptionBudget" => "poddisruptionbudgets".into(),
        _ => format!("{}s", kind.to_lowercase()),
    }
}

/// (kind, name, collection path) for one document.
pub fn target(doc: &Value) -> Result<(String, String, String), String> {
    let api_version = doc
        .get("apiVersion")
        .and_then(Value::as_str)
        .ok_or("document has no apiVersion")?;
    let kind = doc.get("kind").and_then(Value::as_str).ok_or("document has no kind")?;
    let name = doc
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .or_else(|| doc.pointer("/metadata/generateName").and_then(Value::as_str))
        .ok_or_else(|| format!("{kind} has no metadata.name"))?;
    let base = if api_version == "v1" { "/api/v1".to_string() } else { format!("/apis/{api_version}") };
    let path = if CLUSTER_SCOPED.contains(&kind) {
        format!("{base}/{}", plural(kind))
    } else {
        let ns = doc.pointer("/metadata/namespace").and_then(Value::as_str).unwrap_or("default");
        format!("{base}/namespaces/{ns}/{}", plural(kind))
    };
    Ok((kind.to_string(), name.to_string(), path))
}

const APPLY: &str = "/api/plugins/k8s/apply";

pub fn creators() -> Vec<Creator> {
    vec![
        Creator::yaml("k8s:yaml", "Import YAML", APPLY, IMPORT)
            .describe("Any resources, one or more documents — like oc apply -f")
            .at(&["*"]),
        Creator::yaml("k8s:pod", "Pod", APPLY, POD).at(&["#/k8s/pod"]),
        Creator::yaml("k8s:deploy", "Deployment", APPLY, DEPLOYMENT).at(&["#/k8s/deploy"]),
        Creator::yaml("k8s:sts", "StatefulSet", APPLY, STATEFULSET).at(&["#/k8s/sts"]),
        Creator::yaml("k8s:ds", "DaemonSet", APPLY, DAEMONSET).at(&["#/k8s/ds"]),
        Creator::yaml("k8s:job", "Job", APPLY, JOB).at(&["#/k8s/job"]),
        Creator::yaml("k8s:cronjob", "CronJob", APPLY, CRONJOB).at(&["#/k8s/cronjob"]),
        Creator::yaml("k8s:svc", "Service", APPLY, SERVICE).at(&["#/k8s/svc"]),
        Creator::yaml("k8s:pvc", "PersistentVolumeClaim", APPLY, PVC).at(&["#/k8s/pvc"]),
        Creator::yaml("k8s:ns", "Namespace", APPLY, NAMESPACE).at(&["#/k8s/ns"]),
    ]
}

const IMPORT: &str = "# Paste one or more resources; separate documents with ---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: example\n  namespace: default\ndata:\n  key: value\n";

const POD: &str = "apiVersion: v1\nkind: Pod\nmetadata:\n  name: example\n  namespace: default\n  labels:\n    app: example\nspec:\n  containers:\n    - name: app\n      image: registry.gt.lo:5000/busybox:latest\n      command: [\"sh\", \"-c\", \"sleep 3600\"]\n      resources:\n        limits:\n          memory: 64Mi\n";

const DEPLOYMENT: &str = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: example\n  namespace: default\nspec:\n  replicas: 1\n  selector:\n    matchLabels:\n      app: example\n  template:\n    metadata:\n      labels:\n        app: example\n    spec:\n      containers:\n        - name: app\n          image: registry.gt.lo:5000/nginx:latest\n          ports:\n            - containerPort: 80\n";

const STATEFULSET: &str = "apiVersion: apps/v1\nkind: StatefulSet\nmetadata:\n  name: example\n  namespace: default\nspec:\n  serviceName: example\n  replicas: 1\n  selector:\n    matchLabels:\n      app: example\n  template:\n    metadata:\n      labels:\n        app: example\n    spec:\n      containers:\n        - name: app\n          image: registry.gt.lo:5000/nginx:latest\n  volumeClaimTemplates:\n    - metadata:\n        name: data\n      spec:\n        accessModes: [\"ReadWriteOnce\"]\n        resources:\n          requests:\n            storage: 1Gi\n";

const DAEMONSET: &str = "apiVersion: apps/v1\nkind: DaemonSet\nmetadata:\n  name: example\n  namespace: default\nspec:\n  selector:\n    matchLabels:\n      app: example\n  template:\n    metadata:\n      labels:\n        app: example\n    spec:\n      containers:\n        - name: app\n          image: registry.gt.lo:5000/busybox:latest\n          command: [\"sh\", \"-c\", \"sleep 3600\"]\n";

const JOB: &str = "apiVersion: batch/v1\nkind: Job\nmetadata:\n  name: example\n  namespace: default\nspec:\n  template:\n    spec:\n      restartPolicy: Never\n      containers:\n        - name: job\n          image: registry.gt.lo:5000/busybox:latest\n          command: [\"sh\", \"-c\", \"echo done\"]\n";

const CRONJOB: &str = "apiVersion: batch/v1\nkind: CronJob\nmetadata:\n  name: example\n  namespace: default\nspec:\n  schedule: \"*/5 * * * *\"\n  jobTemplate:\n    spec:\n      template:\n        spec:\n          restartPolicy: Never\n          containers:\n            - name: job\n              image: registry.gt.lo:5000/busybox:latest\n              command: [\"sh\", \"-c\", \"date\"]\n";

const SERVICE: &str = "apiVersion: v1\nkind: Service\nmetadata:\n  name: example\n  namespace: default\nspec:\n  selector:\n    app: example\n  ports:\n    - port: 80\n      targetPort: 80\n";

const PVC: &str = "apiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: example\n  namespace: default\nspec:\n  accessModes: [\"ReadWriteOnce\"]\n  resources:\n    requests:\n      storage: 1Gi\n";

const NAMESPACE: &str = "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: example\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_split_and_convert() {
        let docs = parse_documents("apiVersion: v1\nkind: Namespace\nmetadata:\n  name: a\n---\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: d\n  namespace: x\n").unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(target(&docs[0]).unwrap().2, "/api/v1/namespaces");
        assert_eq!(target(&docs[1]).unwrap().2, "/apis/apps/v1/namespaces/x/deployments");
    }

    #[test]
    fn namespaced_defaults_to_default_and_plurals_are_right() {
        let d = parse_documents("apiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata:\n  name: i\n").unwrap();
        assert_eq!(target(&d[0]).unwrap().2, "/apis/networking.k8s.io/v1/namespaces/default/ingresses");
        let d = parse_documents("apiVersion: v1\nkind: Pod\nmetadata:\n  name: p\n  namespace: kube-system\n").unwrap();
        assert_eq!(target(&d[0]).unwrap().2, "/api/v1/namespaces/kube-system/pods");
    }

    #[test]
    fn every_template_parses_to_one_document() {
        for c in creators() {
            let docs = parse_documents(&c.template).unwrap();
            assert_eq!(docs.len(), 1, "{}", c.id);
            target(&docs[0]).unwrap();
        }
    }

    #[test]
    fn missing_fields_are_named() {
        let d = parse_documents("kind: Pod\nmetadata:\n  name: p\n").unwrap();
        assert!(target(&d[0]).unwrap_err().contains("apiVersion"));
        assert!(parse_documents("just: [a\n").is_err());
    }
}

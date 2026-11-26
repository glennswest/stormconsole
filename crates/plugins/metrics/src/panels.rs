//! What is charted, and how an answer becomes a chart. No I/O here.
//!
//! **The queries live here and nowhere else.** The page asks for a level or an object, never for a
//! query: ironprom answers anything to anyone who can reach it, and a free query would read every
//! project's series past the console's access rule. `__NS__` is where a query that names projects
//! takes the viewer's, so it cannot be forgotten: [`query`] refuses to run one without an answer.
//!
//! Series and labels are the exporters' own, read from their code (stormdrive `metrics.rs`, stormblock,
//! rustkube-node `docs/metrics.md`, rustkube `docs/metrics.md`, fastetcd `metrics.rs`, stormvm
//! `metrics.rs`); job names are stormcos's `deploy/metrics/ironprom-node.yml`.

use serde_json::{json, Value};

/// How many lines a chart draws: past this it stops being readable, and says so.
pub const MAX_SERIES: usize = 12;

pub struct Panel {
    pub id: &'static str,
    pub title: &'static str,
    /// celsius, percent, bytes, bytes_per_sec, per_sec, per_min, seconds, cores, updown, flag, mbps, mw, count.
    pub unit: &'static str,
    /// The job that scrapes it, which is what explains an empty chart.
    pub job: &'static str,
    pub query: &'static str,
    /// The labels a line is named by.
    pub legend: &'static [&'static str],
}

pub struct Level {
    pub id: &'static str,
    pub title: &'static str,
    /// What is not here and why, shown on the level.
    pub note: &'static str,
    pub panels: &'static [Panel],
}

const CADVISOR: &str = "kubelet-cadvisor";

// The latency panel leaves out watch and connect, which stay open for as long as they are wanted.
// rustkube spells verbs in lower case and upstream in upper, so both are named.

pub const LEVELS: &[Level] = &[
    Level {
        id: "drives",
        title: "Drives",
        note: "",
        panels: &[
            Panel { id: "temperature", title: "Hottest drives", unit: "celsius", job: "stormdrive",
                query: r#"topk(12, smartctl_device_temperature{job="stormdrive",temperature_type="current"})"#, legend: &["device", "serial"] },
            Panel { id: "wear", title: "Most worn drives", unit: "percent", job: "stormdrive",
                query: r#"topk(12, smartctl_device_percentage_used{job="stormdrive"})"#, legend: &["device", "serial"] },
            Panel { id: "errors", title: "Drive IO errors", unit: "per_sec", job: "stormdrive",
                query: r#"topk(12, rate(stormdrive_drive_io_errors_total{job="stormdrive"}[5m]))"#, legend: &["device", "serial"] },
            // The SAS link (#339, stormdrive#149): loss of DWORD sync per minute, the drives where it moves.
            Panel { id: "link_sync", title: "Drive links: loss of sync", unit: "per_min", job: "stormdrive",
                query: r#"topk(12, sum by (bay, serial, side) (rate(stormdrive_drive_link_errors_total{job="stormdrive",counter="loss_of_sync"}[5m])) * 60 > 0)"#, legend: &["bay", "serial", "side"] },
            Panel { id: "disk_read", title: "Drive reads", unit: "bytes_per_sec", job: "stormdrive",
                query: r#"topk(12, rate(stormdrive_drive_read_bytes_total{job="stormdrive"}[5m]) or rate(smartctl_device_bytes_read{job="stormdrive"}[5m]))"#, legend: &["device", "serial"] },
            Panel { id: "disk_write", title: "Drive writes", unit: "bytes_per_sec", job: "stormdrive",
                query: r#"topk(12, rate(stormdrive_drive_write_bytes_total{job="stormdrive"}[5m]) or rate(smartctl_device_bytes_written{job="stormdrive"}[5m]))"#, legend: &["device", "serial"] },
            Panel { id: "disk_read_ops", title: "Drive read operations", unit: "per_sec", job: "stormdrive",
                query: r#"topk(12, rate(stormdrive_drive_reads_total{job="stormdrive"}[5m]))"#, legend: &["device", "serial"] },
            Panel { id: "disk_write_ops", title: "Drive write operations", unit: "per_sec", job: "stormdrive",
                query: r#"topk(12, rate(stormdrive_drive_writes_total{job="stormdrive"}[5m]))"#, legend: &["device", "serial"] },
        ],
    },
    Level {
        id: "volumes",
        title: "Volumes",
        note: "The engine reports IO by operation, reads and writes apart, across every volume it exports: it has no series per volume and no bytes yet (stormblock#536).",
        panels: &[
            Panel { id: "latency", title: "Volume IO latency, p99", unit: "seconds", job: "stormblock",
                query: r#"histogram_quantile(0.99, sum by (le, op) (rate(stormblock_nvmeof_io_seconds_bucket{job="stormblock"}[5m])))"#, legend: &["op"] },
            Panel { id: "rate", title: "Volume IO rate", unit: "per_sec", job: "stormblock",
                query: r#"sum by (op) (rate(stormblock_nvmeof_io_seconds_count{job="stormblock"}[5m]))"#, legend: &["op"] },
        ],
    },
    Level {
        id: "containers",
        title: "Containers",
        note: "",
        panels: &[
            Panel { id: "cpu", title: "Container CPU", unit: "cores", job: CADVISOR,
                query: r#"topk(10, sum by (namespace, pod, container) (rate(container_cpu_usage_seconds_total{job="kubelet-cadvisor",container!=""__NS__}[5m])))"#,
                legend: &["namespace", "pod", "container"] },
            Panel { id: "memory", title: "Container memory", unit: "bytes", job: CADVISOR,
                query: r#"topk(10, sum by (namespace, pod, container) (container_memory_working_set_bytes{job="kubelet-cadvisor",container!=""__NS__}))"#,
                legend: &["namespace", "pod", "container"] },
            Panel { id: "disk_read", title: "Container disk reads", unit: "bytes_per_sec", job: CADVISOR,
                query: r#"topk(10, sum by (namespace, pod, container) (rate(container_fs_reads_bytes_total{job="kubelet-cadvisor",container!=""__NS__}[5m])))"#,
                legend: &["namespace", "pod", "container"] },
            Panel { id: "disk_write", title: "Container disk writes", unit: "bytes_per_sec", job: CADVISOR,
                query: r#"topk(10, sum by (namespace, pod, container) (rate(container_fs_writes_bytes_total{job="kubelet-cadvisor",container!=""__NS__}[5m])))"#,
                legend: &["namespace", "pod", "container"] },
            Panel { id: "disk_read_ops", title: "Container disk read operations", unit: "per_sec", job: CADVISOR,
                query: r#"topk(10, sum by (namespace, pod, container) (rate(container_fs_reads_total{job="kubelet-cadvisor",container!=""__NS__}[5m])))"#,
                legend: &["namespace", "pod", "container"] },
            Panel { id: "disk_write_ops", title: "Container disk write operations", unit: "per_sec", job: CADVISOR,
                query: r#"topk(10, sum by (namespace, pod, container) (rate(container_fs_writes_total{job="kubelet-cadvisor",container!=""__NS__}[5m])))"#,
                legend: &["namespace", "pod", "container"] },
        ],
    },
    Level {
        id: "node",
        title: "Node",
        note: "This node, from its cadvisor: disks are the whole devices the root cgroup's io.stat names; TCP and UDP are the host's network; scheduler time is every process. There is no per-CPU chart: cgroup v2 keeps no CPU time per CPU, so cadvisor has only the total (cadvisor#40). Hugepages read 0 where none are reserved.",
        panels: &[
            // cadvisor's `/` also has rows for every mounted partition and tmpfs, whose counters read 0
            // (cadvisor#30), so operations are narrowed to the devices io.stat knows.
            Panel { id: "disk_read", title: "Node disk reads", unit: "bytes_per_sec", job: "cadvisor",
                query: r#"sum by (device) (rate(container_fs_reads_bytes_total{job="cadvisor",id="/",device!=""}[5m]))"#, legend: &["device"] },
            Panel { id: "disk_write", title: "Node disk writes", unit: "bytes_per_sec", job: "cadvisor",
                query: r#"sum by (device) (rate(container_fs_writes_bytes_total{job="cadvisor",id="/",device!=""}[5m]))"#, legend: &["device"] },
            Panel { id: "disk_read_ops", title: "Node disk read operations", unit: "per_sec", job: "cadvisor",
                query: r#"sum by (device) (rate(container_fs_reads_total{job="cadvisor",id="/",device!=""}[5m])) and on (device) sum by (device) (container_fs_reads_bytes_total{job="cadvisor",id="/",device!=""})"#, legend: &["device"] },
            Panel { id: "disk_write_ops", title: "Node disk write operations", unit: "per_sec", job: "cadvisor",
                query: r#"sum by (device) (rate(container_fs_writes_total{job="cadvisor",id="/",device!=""}[5m])) and on (device) sum by (device) (container_fs_writes_bytes_total{job="cadvisor",id="/",device!=""})"#, legend: &["device"] },
            // cadvisor's optional groups (#386, cadvisor#40). `/` is the node: its network is the host's
            // (pid 1's), and its schedstat is every process on it. IPv4 and IPv6 are one line per state.
            Panel { id: "tcp", title: "TCP connections, by state", unit: "count", job: "cadvisor",
                query: r#"sum by (tcp_state) ({__name__=~"container_network_tcp6?_usage_total",job="cadvisor",id="/"})"#, legend: &["tcp_state"] },
            Panel { id: "udp_sockets", title: "UDP sockets", unit: "count", job: "cadvisor",
                query: r#"sum by (udp_state) ({__name__=~"container_network_udp6?_usage_total",job="cadvisor",id="/",udp_state="listen"})"#, legend: &["udp_state"] },
            Panel { id: "udp_queued", title: "UDP bytes queued, receive and send", unit: "bytes", job: "cadvisor",
                query: r#"sum by (udp_state) ({__name__=~"container_network_udp6?_usage_total",job="cadvisor",id="/",udp_state=~"rxqueued|txqueued"})"#, legend: &["udp_state"] },
            // The kernel counts drops per socket, so a closed socket's are gone: this is the open sockets' total.
            Panel { id: "udp_dropped", title: "UDP packets dropped, on open sockets", unit: "count", job: "cadvisor",
                query: r#"sum by (udp_state) ({__name__=~"container_network_udp6?_usage_total",job="cadvisor",id="/",udp_state="dropped"})"#, legend: &["udp_state"] },
            Panel { id: "sched_wait", title: "CPU time spent waiting to run", unit: "cores", job: "cadvisor",
                query: r#"sum(rate(container_cpu_schedstat_runqueue_seconds_total{job="cadvisor",id="/"}[5m]))"#, legend: &[] },
            Panel { id: "sched_latency", title: "Scheduler latency: average wait per timeslice", unit: "seconds", job: "cadvisor",
                query: r#"sum(rate(container_cpu_schedstat_runqueue_seconds_total{job="cadvisor",id="/"}[5m])) / sum(rate(container_cpu_schedstat_run_periods_total{job="cadvisor",id="/"}[5m]))"#, legend: &[] },
            // The top-level cgroups (services, kubepods), which hold every pod's time without naming a project.
            Panel { id: "sched_wait_by", title: "Waiting to run, by top-level cgroup", unit: "cores", job: "cadvisor",
                query: r#"topk(10, rate(container_cpu_schedstat_runqueue_seconds_total{job="cadvisor",id=~"/[^/]+"}[5m]))"#, legend: &["id"] },
            // cgroup v2's root has no hugetlb files, so the node's use is the top-level cgroups' (each holds its children's).
            Panel { id: "hugetlb", title: "Hugepages in use, by page size", unit: "bytes", job: "cadvisor",
                query: r#"sum by (pagesize) (container_hugetlb_usage_bytes{job="cadvisor",id=~"/[^/]+"})"#, legend: &["pagesize"] },
            Panel { id: "hugetlb_failures", title: "Hugepage allocations refused at a limit", unit: "per_min", job: "cadvisor",
                query: r#"sum by (pagesize) (rate(container_hugetlb_failcnt{job="cadvisor",id=~"/[^/]+"}[5m])) * 60"#, legend: &["pagesize"] },
        ],
    },
    Level {
        id: "apiserver",
        title: "API server",
        note: "",
        panels: &[
            Panel { id: "rate", title: "Request rate", unit: "per_sec", job: "apiserver",
                query: r#"sum by (verb) (rate(apiserver_request_total{job="apiserver"}[5m]))"#, legend: &["verb"] },
            Panel { id: "latency", title: "Request latency, p99", unit: "seconds", job: "apiserver",
                query: r#"histogram_quantile(0.99, sum by (le, verb) (rate(apiserver_request_duration_seconds_bucket{job="apiserver",verb!~"watch|connect|WATCH|CONNECT"}[5m])))"#, legend: &["verb"] },
        ],
    },
    Level {
        id: "datastore",
        title: "Datastore",
        note: "",
        panels: &[
            Panel { id: "size", title: "Database size", unit: "bytes", job: "fastetcd",
                query: r#"etcd_mvcc_db_total_size_in_bytes{job="fastetcd"}"#, legend: &["instance"] },
            Panel { id: "puts", title: "Puts", unit: "per_sec", job: "fastetcd",
                query: r#"rate(etcd_debugging_mvcc_put_total{job="fastetcd"}[5m])"#, legend: &["instance"] },
            Panel { id: "ranges", title: "Ranges", unit: "per_sec", job: "fastetcd",
                query: r#"rate(etcd_debugging_mvcc_range_total{job="fastetcd"}[5m])"#, legend: &["instance"] },
            Panel { id: "get", title: "Single-key read, p99", unit: "seconds", job: "fastetcd",
                query: r#"histogram_quantile(0.99, sum by (le, cache) (rate(fastetcd_mvcc_get_duration_seconds_bucket{job="fastetcd"}[5m])))"#, legend: &["cache"] },
        ],
    },
    Level {
        id: "vms",
        title: "Virtual machines",
        note: "",
        panels: &[
            Panel { id: "cpu", title: "VM CPU", unit: "cores", job: "stormvm",
                query: r#"topk(10, sum by (namespace, name) (rate(kubevirt_vmi_vcpu_seconds_total{job="stormvm"__NS__}[5m])))"#, legend: &["namespace", "name"] },
            Panel { id: "read", title: "VM disk reads", unit: "bytes_per_sec", job: "stormvm",
                query: r#"topk(10, sum by (namespace, name) (rate(kubevirt_vmi_storage_read_traffic_bytes_total{job="stormvm"__NS__}[5m])))"#, legend: &["namespace", "name"] },
            Panel { id: "write", title: "VM disk writes", unit: "bytes_per_sec", job: "stormvm",
                query: r#"topk(10, sum by (namespace, name) (rate(kubevirt_vmi_storage_write_traffic_bytes_total{job="stormvm"__NS__}[5m])))"#, legend: &["namespace", "name"] },
            Panel { id: "read_ops", title: "VM disk read operations", unit: "per_sec", job: "stormvm",
                query: r#"topk(10, sum by (namespace, name) (rate(kubevirt_vmi_storage_iops_read_total{job="stormvm"__NS__}[5m])))"#, legend: &["namespace", "name"] },
            Panel { id: "write_ops", title: "VM disk write operations", unit: "per_sec", job: "stormvm",
                query: r#"topk(10, sum by (namespace, name) (rate(kubevirt_vmi_storage_iops_write_total{job="stormvm"__NS__}[5m])))"#, legend: &["namespace", "name"] },
        ],
    },
    Level {
        id: "network",
        title: "Network",
        note: "",
        panels: &[
            // This node's host ports, from stormport (#336, stormpump#236): one line per interface.
            Panel { id: "port_link", title: "Host port links (1 up, 0 down)", unit: "updown", job: "stormport",
                query: r#"storm_port_link_up{job="stormport"}"#, legend: &["ifname"] },
            Panel { id: "port_fec_uncorrectable", title: "Host ports: FEC uncorrectable blocks", unit: "per_sec", job: "stormport",
                query: r#"topk(12, rate(storm_port_fec_uncorrectable_blocks_total{job="stormport"}[5m]))"#, legend: &["ifname"] },
            Panel { id: "port_errors", title: "Host ports: errors, rx and tx", unit: "per_sec", job: "stormport",
                query: r#"topk(12, sum by (ifname) (rate(storm_port_rx_errors_total{job="stormport"}[5m]) + rate(storm_port_tx_errors_total{job="stormport"}[5m])))"#, legend: &["ifname"] },
            Panel { id: "port_rx", title: "Host ports: received", unit: "bytes_per_sec", job: "stormport",
                query: r#"rate(storm_port_rx_bytes_total{job="stormport"}[5m])"#, legend: &["ifname"] },
            Panel { id: "port_tx", title: "Host ports: sent", unit: "bytes_per_sec", job: "stormport",
                query: r#"rate(storm_port_tx_bytes_total{job="stormport"}[5m])"#, legend: &["ifname"] },
            Panel { id: "drops", title: "Packets dropped", unit: "per_sec", job: "cilium-agent",
                query: r#"topk(10, sum by (reason, direction) (rate(cilium_drop_count_total{job="cilium-agent"}[5m])))"#, legend: &["reason", "direction"] },
        ],
    },
];

const DRIVE: &[Panel] = &[
    // The SAS link, both sides (#339, stormdrive#149): the drive page's Link trend.
    Panel { id: "link_sync", title: "Link: loss of sync", unit: "per_min", job: "stormdrive",
        query: r#"sum by (side) (rate(stormdrive_drive_link_errors_total{job="stormdrive",counter="loss_of_sync",__THE__}[5m])) * 60"#, legend: &["side"] },
    Panel { id: "link_errors", title: "Link: every counter", unit: "per_min", job: "stormdrive",
        query: r#"sum by (side, counter) (rate(stormdrive_drive_link_errors_total{job="stormdrive",__THE__}[5m])) * 60"#, legend: &["side", "counter"] },
    Panel { id: "temperature", title: "Temperature", unit: "celsius", job: "stormdrive",
        query: r#"smartctl_device_temperature{job="stormdrive",temperature_type="current",__THE__}"#, legend: &["device"] },
    Panel { id: "wear", title: "Wear", unit: "percent", job: "stormdrive",
        query: r#"smartctl_device_percentage_used{job="stormdrive",__THE__}"#, legend: &["device"] },
    Panel { id: "errors", title: "IO errors", unit: "per_sec", job: "stormdrive",
        query: r#"rate(stormdrive_drive_io_errors_total{job="stormdrive",__THE__}[5m])"#, legend: &["device"] },
    Panel { id: "disk_read", title: "Reads", unit: "bytes_per_sec", job: "stormdrive",
        query: r#"rate(stormdrive_drive_read_bytes_total{job="stormdrive",__THE__}[5m]) or rate(smartctl_device_bytes_read{job="stormdrive",__THE__}[5m])"#, legend: &["device"] },
    Panel { id: "disk_write", title: "Writes", unit: "bytes_per_sec", job: "stormdrive",
        query: r#"rate(stormdrive_drive_write_bytes_total{job="stormdrive",__THE__}[5m]) or rate(smartctl_device_bytes_written{job="stormdrive",__THE__}[5m])"#, legend: &["device"] },
    Panel { id: "disk_read_ops", title: "Read operations", unit: "per_sec", job: "stormdrive",
        query: r#"rate(stormdrive_drive_reads_total{job="stormdrive",__THE__}[5m])"#, legend: &["device"] },
    Panel { id: "disk_write_ops", title: "Write operations", unit: "per_sec", job: "stormdrive",
        query: r#"rate(stormdrive_drive_writes_total{job="stormdrive",__THE__}[5m])"#, legend: &["device"] },
];

const POD: &[Panel] = &[
    Panel { id: "cpu", title: "CPU", unit: "cores", job: CADVISOR,
        query: r#"sum by (container) (rate(container_cpu_usage_seconds_total{job="kubelet-cadvisor",container!="",__THE__}[5m]))"#, legend: &["container"] },
    Panel { id: "memory", title: "Memory", unit: "bytes", job: CADVISOR,
        query: r#"sum by (container) (container_memory_working_set_bytes{job="kubelet-cadvisor",container!="",__THE__})"#, legend: &["container"] },
    Panel { id: "rx", title: "Network received", unit: "bytes_per_sec", job: CADVISOR,
        query: r#"sum by (interface) (rate(container_network_receive_bytes_total{job="kubelet-cadvisor",__THE__}[5m]))"#, legend: &["interface"] },
    Panel { id: "tx", title: "Network sent", unit: "bytes_per_sec", job: CADVISOR,
        query: r#"sum by (interface) (rate(container_network_transmit_bytes_total{job="kubelet-cadvisor",__THE__}[5m]))"#, legend: &["interface"] },
    Panel { id: "disk_read", title: "Disk reads", unit: "bytes_per_sec", job: CADVISOR,
        query: r#"sum by (container) (rate(container_fs_reads_bytes_total{job="kubelet-cadvisor",container!="",__THE__}[5m]))"#, legend: &["container"] },
    Panel { id: "disk_write", title: "Disk writes", unit: "bytes_per_sec", job: CADVISOR,
        query: r#"sum by (container) (rate(container_fs_writes_bytes_total{job="kubelet-cadvisor",container!="",__THE__}[5m]))"#, legend: &["container"] },
    Panel { id: "disk_read_ops", title: "Disk read operations", unit: "per_sec", job: CADVISOR,
        query: r#"sum by (container) (rate(container_fs_reads_total{job="kubelet-cadvisor",container!="",__THE__}[5m]))"#, legend: &["container"] },
    Panel { id: "disk_write_ops", title: "Disk write operations", unit: "per_sec", job: CADVISOR,
        query: r#"sum by (container) (rate(container_fs_writes_total{job="kubelet-cadvisor",container!="",__THE__}[5m]))"#, legend: &["container"] },
];

const VM: &[Panel] = &[
    Panel { id: "cpu", title: "CPU", unit: "cores", job: "stormvm",
        query: r#"sum by (id) (rate(kubevirt_vmi_vcpu_seconds_total{job="stormvm",__THE__}[5m]))"#, legend: &["id"] },
    // "Is it busy or stuck": time it could have run and did not get. Not a share of the guest's own CPU.
    Panel { id: "wait", title: "CPU waiting for the host", unit: "cores", job: "stormvm",
        query: r#"sum by (id) (rate(kubevirt_vmi_vcpu_delay_seconds_total{job="stormvm",__THE__}[5m]))"#, legend: &["id"] },
    // Allocated against touched (#14). The allocation is the hypervisor's, because a VM's cgroup has no memory.max
    // and so cadvisor would read it as unlimited (cadvisor#15).
    Panel { id: "memory", title: "Memory: allocated and in use", unit: "bytes", job: "stormvm",
        query: r#"{__name__=~"kubevirt_vmi_memory_(domain|resident|actual_balloon)_bytes",job="stormvm",__THE__}"#, legend: &["__name__"] },
    Panel { id: "read", title: "Disk reads", unit: "bytes_per_sec", job: "stormvm",
        query: r#"sum by (drive) (rate(kubevirt_vmi_storage_read_traffic_bytes_total{job="stormvm",__THE__}[5m]))"#, legend: &["drive"] },
    Panel { id: "write", title: "Disk writes", unit: "bytes_per_sec", job: "stormvm",
        query: r#"sum by (drive) (rate(kubevirt_vmi_storage_write_traffic_bytes_total{job="stormvm",__THE__}[5m]))"#, legend: &["drive"] },
    Panel { id: "read_ops", title: "Disk read operations", unit: "per_sec", job: "stormvm",
        query: r#"sum by (drive) (rate(kubevirt_vmi_storage_iops_read_total{job="stormvm",__THE__}[5m]))"#, legend: &["drive"] },
    Panel { id: "write_ops", title: "Disk write operations", unit: "per_sec", job: "stormvm",
        query: r#"sum by (drive) (rate(kubevirt_vmi_storage_iops_write_total{job="stormvm",__THE__}[5m]))"#, legend: &["drive"] },
    Panel { id: "rx", title: "Network received", unit: "bytes_per_sec", job: "stormvm",
        query: r#"sum by (interface) (rate(kubevirt_vmi_network_receive_bytes_total{job="stormvm",__THE__}[5m]))"#, legend: &["interface"] },
    Panel { id: "tx", title: "Network sent", unit: "bytes_per_sec", job: "stormvm",
        query: r#"sum by (interface) (rate(kubevirt_vmi_network_transmit_bytes_total{job="stormvm",__THE__}[5m]))"#, legend: &["interface"] },
    Panel { id: "rx_packets", title: "Packets received", unit: "per_sec", job: "stormvm",
        query: r#"sum by (interface) (rate(kubevirt_vmi_network_receive_packets_total{job="stormvm",__THE__}[5m]))"#, legend: &["interface"] },
    Panel { id: "tx_packets", title: "Packets sent", unit: "per_sec", job: "stormvm",
        query: r#"sum by (interface) (rate(kubevirt_vmi_network_transmit_packets_total{job="stormvm",__THE__}[5m]))"#, legend: &["interface"] },
];

// One host port (#336): stormport's `storm_port_*` (stormpump `crates/stormport`), labelled `node`, `ifname`
// and `mac`. Each counter is its own series, so a chart of several is several rates, each named in `counter`.
const PORT: &[Panel] = &[
    Panel { id: "link", title: "Link (1 up, 0 down)", unit: "updown", job: "stormport",
        query: r#"storm_port_link_up{job="stormport",__THE__}"#, legend: &["ifname"] },
    Panel { id: "fec_mode", title: "FEC mode in use (1 while in use)", unit: "flag", job: "stormport",
        query: r#"storm_port_fec_active{job="stormport",__THE__}"#, legend: &["fec"] },
    Panel { id: "fec_blocks", title: "FEC blocks", unit: "per_sec", job: "stormport",
        query: r#"label_replace(rate(storm_port_fec_corrected_blocks_total{job="stormport",__THE__}[5m]), "counter", "corrected", "", "") or label_replace(rate(storm_port_fec_uncorrectable_blocks_total{job="stormport",__THE__}[5m]), "counter", "uncorrectable", "", "")"#,
        legend: &["counter"] },
    Panel { id: "errors", title: "Errors", unit: "per_sec", job: "stormport",
        query: r#"label_replace(rate(storm_port_rx_errors_total{job="stormport",__THE__}[5m]), "counter", "rx errors", "", "") or label_replace(rate(storm_port_tx_errors_total{job="stormport",__THE__}[5m]), "counter", "tx errors", "", "") or label_replace(rate(storm_port_rx_crc_errors_total{job="stormport",__THE__}[5m]), "counter", "rx CRC", "", "") or label_replace(rate(storm_port_rx_missed_errors_total{job="stormport",__THE__}[5m]), "counter", "rx missed", "", "")"#,
        legend: &["counter"] },
    Panel { id: "drops", title: "Drops", unit: "per_sec", job: "stormport",
        query: r#"label_replace(rate(storm_port_rx_dropped_total{job="stormport",__THE__}[5m]), "counter", "rx", "", "") or label_replace(rate(storm_port_tx_dropped_total{job="stormport",__THE__}[5m]), "counter", "tx", "", "")"#,
        legend: &["counter"] },
    Panel { id: "traffic", title: "Traffic", unit: "bytes_per_sec", job: "stormport",
        query: r#"label_replace(rate(storm_port_rx_bytes_total{job="stormport",__THE__}[5m]), "counter", "received", "", "") or label_replace(rate(storm_port_tx_bytes_total{job="stormport",__THE__}[5m]), "counter", "sent", "", "")"#,
        legend: &["counter"] },
    Panel { id: "packets", title: "Packets", unit: "per_sec", job: "stormport",
        query: r#"label_replace(rate(storm_port_rx_packets_total{job="stormport",__THE__}[5m]), "counter", "received", "", "") or label_replace(rate(storm_port_tx_packets_total{job="stormport",__THE__}[5m]), "counter", "sent", "", "")"#,
        legend: &["counter"] },
    Panel { id: "pause", title: "Pause frames", unit: "per_sec", job: "stormport",
        query: r#"label_replace(rate(storm_port_pause_rx_frames_total{job="stormport",__THE__}[5m]), "counter", "rx", "", "") or label_replace(rate(storm_port_pause_tx_frames_total{job="stormport",__THE__}[5m]), "counter", "tx", "", "")"#,
        legend: &["counter"] },
    Panel { id: "speed", title: "Speed", unit: "mbps", job: "stormport",
        query: r#"storm_port_speed_mbps{job="stormport",__THE__}"#, legend: &["ifname"] },
    Panel { id: "module_temperature", title: "Module temperature", unit: "celsius", job: "stormport",
        query: r#"storm_port_module_temperature_celsius{job="stormport",__THE__}"#, legend: &["ifname"] },
    Panel { id: "module_power", title: "Module optical power", unit: "mw", job: "stormport",
        query: r#"label_replace(storm_port_module_rx_power_mw{job="stormport",__THE__}, "dir", "rx", "", "") or label_replace(storm_port_module_tx_power_mw{job="stormport",__THE__}, "dir", "tx", "", "")"#,
        legend: &["dir", "lane"] },
];

// A RAID set's integrity (#385, stormraid#80): stormraid's `stormraid_member_*` and `stormraid_set_*` on
// 127.0.0.1:9470, scraped as job `stormraid` (stormcos#1466). Labelled `node, set, drive, bay, slot, member`;
// `drive` is the member's WWN, else its path. Counters of rare events, so a chart is the count itself:
// a step is when it rose, which a per-second rate of 0.0003 does not say.
const ERRORS: &str = r#"label_replace(sum by (slot, drive) (stormraid_member_pi_errors_total{job="stormraid",__THE__}), "counter", "PI", "", "") or label_replace(sum by (slot, drive) (stormraid_member_read_errors_total{job="stormraid",__THE__}), "counter", "read", "", "") or label_replace(sum by (slot, drive) (stormraid_member_write_errors_total{job="stormraid",__THE__}), "counter", "write", "", "")"#;

const RAIDSET: &[Panel] = &[
    Panel { id: "integrity_mismatches", title: "Checksum mismatches, by where found", unit: "count", job: "stormraid",
        query: r#"sum by (level) (stormraid_member_checksum_mismatches_total{job="stormraid",__THE__})"#, legend: &["level"] },
    Panel { id: "integrity_member_mismatches", title: "Checksum mismatches, per member", unit: "count", job: "stormraid",
        query: r#"sum by (slot, drive) (stormraid_member_checksum_mismatches_total{job="stormraid",__THE__})"#, legend: &["slot", "drive"] },
    Panel { id: "integrity_repaired", title: "Blocks rewritten from parity, per member", unit: "count", job: "stormraid",
        query: r#"sum by (slot, drive) (stormraid_member_blocks_repaired_total{job="stormraid",__THE__})"#, legend: &["slot", "drive"] },
    Panel { id: "integrity_errors", title: "PI, read and write errors, per member", unit: "count", job: "stormraid",
        query: ERRORS, legend: &["slot", "counter"] },
    Panel { id: "integrity_unrecoverable", title: "Rows past parity", unit: "count", job: "stormraid",
        query: r#"stormraid_set_unrecoverable_rows_total{job="stormraid",__THE__}"#, legend: &["set"] },
    Panel { id: "integrity_scrubs", title: "Scrub passes done", unit: "count", job: "stormraid",
        query: r#"stormraid_scrub_passes_total{job="stormraid",__THE__}"#, legend: &["set"] },
];

// One member of a set, the drive page's Integrity trend: by set and slot, which the RaidSet's member row and
// stormraid's series both carry. The WWN does not do: the object and the label spell it each their own way.
// A slot's drive is named on its line, so a replaced drive's history and its replacement's are two lines.
const MEMBER: &[Panel] = &[
    Panel { id: "integrity_mismatches", title: "Checksum mismatches, by where found", unit: "count", job: "stormraid",
        query: r#"sum by (level, drive) (stormraid_member_checksum_mismatches_total{job="stormraid",__THE__})"#, legend: &["level", "drive"] },
    Panel { id: "integrity_repaired", title: "Blocks rewritten from parity", unit: "count", job: "stormraid",
        query: r#"sum by (drive) (stormraid_member_blocks_repaired_total{job="stormraid",__THE__})"#, legend: &["drive"] },
    Panel { id: "integrity_errors", title: "PI, read and write errors", unit: "count", job: "stormraid",
        query: ERRORS, legend: &["counter", "drive"] },
];

/// Why a disk chart is empty when its source has yet to report it: the series is not
/// there at all, which reads as a fault without this. `None` for every other chart.
pub fn unreported(p: &Panel) -> Option<String> {
    if !p.id.starts_with("disk_") {
        return None;
    }
    match p.job {
        CADVISOR => Some("The node's kubelet does not report a container's disk reads and writes yet (rustkube-node#323).".into()),
        "stormdrive" if p.id.ends_with("_ops") => Some("stormdrive does not report a drive's operations yet (stormdrive#120).".into()),
        "stormdrive" => Some("Only NVMe drives report this so far, as a SMART counter; other kinds wait on stormdrive#120.".into()),
        _ => None,
    }
}

/// One object's series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entity {
    Drive { serial: String },
    Pod { ns: String, name: String },
    Vm { ns: String, name: String },
    /// A node's host port (#336).
    Port { node: String, ifname: String },
    /// A RAID set on its node (#385).
    Raidset { node: String, set: String },
    /// One member of a set, by its slot (#385).
    Member { set: String, slot: u32 },
}

impl Entity {
    pub fn panels(&self) -> &'static [Panel] {
        match self {
            Entity::Drive { .. } => DRIVE,
            Entity::Pod { .. } => POD,
            Entity::Vm { .. } => VM,
            Entity::Port { .. } => PORT,
            Entity::Raidset { .. } => RAIDSET,
            Entity::Member { .. } => MEMBER,
        }
    }

    /// The project it is in, which decides whether this viewer may see it.
    pub fn namespace(&self) -> Option<&str> {
        match self {
            Entity::Drive { .. } | Entity::Port { .. } | Entity::Raidset { .. } | Entity::Member { .. } => None,
            Entity::Pod { ns, .. } | Entity::Vm { ns, .. } => Some(ns),
        }
    }

    fn selector(&self) -> String {
        match self {
            Entity::Drive { serial } => format!("serial={}", quote(serial)),
            Entity::Pod { ns, name } => format!("namespace={},pod={}", quote(ns), quote(name)),
            Entity::Vm { ns, name } => format!("namespace={},name={}", quote(ns), quote(name)),
            Entity::Port { node, ifname } => format!("node={},ifname={}", quote(node), quote(ifname)),
            // ironprom keeps a target's own `node` (stormcos `ironprom-node.yml`), and a set's name is the cluster's.
            Entity::Raidset { set, .. } => format!("set={}", quote(set)),
            Entity::Member { set, slot } => format!("set={},slot={}", quote(set), quote(&slot.to_string())),
        }
    }

    /// What is not here and why, shown above its charts.
    pub fn note(&self) -> &'static str {
        match self {
            // stormvm `metrics.rs`: the counters are the tap's, and these two have none.
            Entity::Vm { .. } => "An interface on qemu's userspace network or passt has no kernel interface, so it has no network series.",
            // stormpump docs/ports.md: a BASE-T port has no module and no FEC.
            Entity::Port { .. } => "A BASE-T port has no module and no FEC, so those charts stay empty for it. Per-queue counters are not served yet.",
            // stormraid's counters are kept with the set, so each line is the count since the set was made.
            Entity::Raidset { .. } | Entity::Member { .. } => "Each line is the count since the set was made: a step is when it rose.",
            _ => "",
        }
    }

    pub fn title(&self) -> String {
        match self {
            Entity::Drive { serial } => format!("Drive {serial}"),
            Entity::Pod { ns, name } => format!("Pod {name} in {ns}"),
            Entity::Vm { ns, name } => format!("Virtual machine {name} in {ns}"),
            Entity::Port { node, ifname } => format!("Port {ifname} on {node}"),
            Entity::Raidset { node, set } => format!("RAID set {set} on {node}"),
            Entity::Member { set, slot } => format!("Slot {slot} of RAID set {set}"),
        }
    }
}

/// A PromQL string. A serial comes from a drive, a name from a URL: neither may end the string.
fn quote(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The query for this panel: for this object, within these projects (`None`: every project).
/// `Err` when the viewer has no project to read it in, which is an answer and not a failure.
pub fn query(panel: &Panel, entity: Option<&Entity>, visible: Option<&[String]>) -> Result<String, String> {
    let mut q = panel.query.to_string();
    if let Some(e) = entity {
        q = q.replace("__THE__", &e.selector());
    }
    if q.contains("__NS__") {
        let matcher = match visible {
            None => String::new(),
            Some(names) => {
                // A project's name is a DNS label. Anything else is left out, not escaped.
                let names: Vec<&str> = names
                    .iter()
                    .map(String::as_str)
                    .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
                    .collect();
                if names.is_empty() {
                    return Err("none of your projects is visible here".to_string());
                }
                format!(r#",namespace=~"{}""#, names.join("|"))
            }
        };
        q = q.replace("__NS__", &matcher);
    }
    Ok(q)
}

/// A line's name, from the labels the panel names: what the series has, joined.
fn line_name(metric: &Value, legend: &[&str]) -> String {
    let parts: Vec<&str> = legend.iter().filter_map(|l| metric.get(*l)?.as_str()).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        "total".to_string()
    } else {
        parts.join(" / ")
    }
}

/// A sample. `NaN` is what a quantile answers with no traffic, and it is a gap and not a value.
fn sample(pair: &Value) -> Option<Value> {
    let t = pair.get(0)?.as_f64()?;
    let v = pair.get(1)?.as_str()?.parse::<f64>().ok().filter(|v| v.is_finite());
    Some(json!({"t": (t * 1000.0) as i64, "v": v}))
}

/// `data.result` of a range query as lines, each named and in a fixed order, so a line keeps its colour
/// from one refresh to the next. Returns the lines and how many were left out.
pub fn lines(result: &[Value], legend: &[&str]) -> (Vec<Value>, usize) {
    let mut named: Vec<(String, Vec<Value>)> = result
        .iter()
        .map(|s| {
            let name = line_name(s.get("metric").unwrap_or(&Value::Null), legend);
            // A panel of several series is named by what each one is.
            let name = match name.as_str() {
                "kubevirt_vmi_memory_domain_bytes" => "allocated".to_string(),
                "kubevirt_vmi_memory_resident_bytes" => "in use on the node".to_string(),
                "kubevirt_vmi_memory_actual_balloon_bytes" => "the balloon leaves the guest".to_string(),
                _ => name,
            };
            let points = s.get("values").and_then(Value::as_array).map(|a| a.iter().filter_map(sample).collect()).unwrap_or_default();
            (name, points)
        })
        .collect();
    named.sort_by(|a, b| a.0.cmp(&b.0));
    let over = named.len().saturating_sub(MAX_SERIES);
    named.truncate(MAX_SERIES);
    let out = named.into_iter().enumerate().map(|(i, (name, points))| json!({"name": name, "slot": i, "points": points})).collect();
    (out, over)
}

/// How far apart the points of a window are: about 120 of them, and no finer than a scrape.
pub fn step_for(window: i64) -> i64 {
    (window / 120).max(30)
}

/// What the page may ask for: from 15 minutes to 90 days, which is how long ironprom keeps the integrity
/// series (ironprom#53); the rest it keeps 15 or 30 days, and a longer window shows that as no line (#385).
pub fn clamp_window(window: Option<i64>) -> i64 {
    window.unwrap_or(3600).clamp(900, 90 * 86400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vms_memory_lines_are_named_by_what_they_are() {
        let series = |n: &str| json!({"metric": {"__name__": n, "namespace": "a", "name": "b"}, "values": [[1, "1"]]});
        let (lines, _) = lines(
            &[series("kubevirt_vmi_memory_domain_bytes"), series("kubevirt_vmi_memory_resident_bytes"), series("kubevirt_vmi_memory_actual_balloon_bytes")],
            &["__name__"],
        );
        let names: Vec<&str> = lines.iter().map(|l| l["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["allocated", "in use on the node", "the balloon leaves the guest"]);
    }

    fn panel(level: &str, id: &str) -> &'static Panel {
        LEVELS.iter().find(|l| l.id == level).unwrap().panels.iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn reads_and_writes_are_charted_apart_at_every_level_that_has_disks() {
        let ids = |ps: &[Panel]| ps.iter().map(|p| p.id).collect::<Vec<_>>();
        for level in ["drives", "containers", "node"] {
            let l = LEVELS.iter().find(|l| l.id == level).unwrap();
            for want in ["disk_read", "disk_write", "disk_read_ops", "disk_write_ops"] {
                assert!(ids(l.panels).contains(&want), "{level} has no {want}");
            }
        }
        for ps in [DRIVE, POD] {
            for want in ["disk_read", "disk_write", "disk_read_ops", "disk_write_ops"] {
                assert!(ids(ps).contains(&want), "{want}");
            }
        }
        // Each is one direction: no panel mixes them.
        for p in LEVELS.iter().flat_map(|l| l.panels).chain(DRIVE).chain(POD).filter(|p| p.id.starts_with("disk_")) {
            let (reads, writes) = (p.query.contains("read"), p.query.contains("write"));
            assert!(reads != writes, "{}", p.query);
            assert_eq!(reads, p.id.contains("read"), "{}", p.query);
        }
    }

    #[test]
    fn an_empty_disk_chart_names_what_it_waits_on() {
        let of = |level: &str, id: &str| unreported(panel(level, id)).unwrap_or_default();
        assert!(of("containers", "disk_read").contains("rustkube-node#"));
        assert!(of("drives", "disk_read").contains("NVMe"));
        assert!(of("drives", "disk_write_ops").contains("stormdrive#"));
        assert_eq!(of("node", "disk_read"), "", "cadvisor reports these: empty there is the usual sentence");
        assert_eq!(of("drives", "errors"), "");
    }

    #[test]
    fn every_query_is_filled_and_scraped_by_its_job() {
        let e = Entity::Pod { ns: "a".into(), name: "b".into() };
        for p in LEVELS.iter().flat_map(|l| l.panels) {
            let q = query(p, None, None).unwrap();
            assert!(!q.replace("__name__", "").contains("__"), "{q}");
            assert!(q.contains(&format!(r#"job="{}""#, p.job)), "{q}");
        }
        for p in [DRIVE, POD, VM, PORT, RAIDSET, MEMBER].into_iter().flatten() {
            let q = query(p, Some(&e), None).unwrap();
            assert!(!q.contains("__THE__"), "{q}");
            assert!(q.contains(&format!(r#"job="{}""#, p.job)), "{q}");
        }
    }

    #[test]
    fn the_node_charts_cadvisors_optional_groups_node_wide() {
        // #386 (cadvisor#40): tcp, udp, sched and hugetlb, from the node's own cadvisor, never a project's cgroup.
        for id in ["tcp", "udp_sockets", "udp_queued", "udp_dropped", "sched_wait", "sched_latency", "sched_wait_by", "hugetlb", "hugetlb_failures"] {
            let p = panel("node", id);
            assert_eq!(p.job, "cadvisor", "{id}");
            assert!(p.query.contains(r#"id="/""#) || p.query.contains(r#"id=~"/[^/]+""#), "{id}: {}", p.query);
            assert!(!p.query.contains("__NS__") && !p.query.contains("namespace"), "{id}");
        }
        // Both address families, one line per state.
        for id in ["tcp", "udp_sockets", "udp_queued", "udp_dropped"] {
            assert!(panel("node", id).query.contains(r#"__name__=~"container_network_"#), "{id}");
            assert!(panel("node", id).query.contains(r#"6?_usage_total""#), "{id}");
        }
        assert_eq!(panel("node", "tcp").legend, ["tcp_state"]);
        // The latency is waiting over timeslices, both rates of `/`.
        let l = panel("node", "sched_latency").query;
        assert!(l.contains("runqueue_seconds_total") && l.contains("run_periods_total") && l.contains(" / "), "{l}");
        // The root has no hugetlb files on cgroup v2: the top-level cgroups, each counted once.
        assert!(panel("node", "hugetlb").query.contains(r#"id=~"/[^/]+""#));
        // No per-CPU chart, and the level says why.
        let node = LEVELS.iter().find(|l| l.id == "node").unwrap();
        assert!(!node.panels.iter().any(|p| p.query.contains(r#"cpu!="total""#) || p.id.contains("percpu")));
        assert!(node.note.contains("per-CPU") && node.note.contains("cadvisor#40"), "{}", node.note);
    }

    #[test]
    fn a_project_series_carries_the_viewers_projects() {
        let p = panel("containers", "cpu");
        let every = query(p, None, None).unwrap();
        assert!(!every.contains("namespace=~"), "{every}");
        let two = vec!["shop".to_string(), "lab-1".to_string()];
        let q = query(p, None, Some(&two)).unwrap();
        assert!(q.contains(r#"container!="",namespace=~"shop|lab-1"}"#), "{q}");
        assert!(query(p, None, Some(&[])).is_err(), "no project: nothing is asked");
        let odd = vec![r#"x"}|up{a="#.to_string()];
        assert!(query(p, None, Some(&odd)).is_err(), "a name that is not a project's is left out");
        // A level with no project in it is not narrowed, and not refused.
        assert!(query(panel("drives", "wear"), None, Some(&[])).is_ok());
    }

    #[test]
    fn every_panel_over_projects_is_narrowed() {
        // Containers and VMs carry a namespace; the rest of the levels' series do not.
        for l in LEVELS {
            for p in l.panels {
                assert_eq!(p.query.contains("__NS__"), p.query.contains("namespace,"), "{}", p.query);
            }
        }
    }

    /// A set's and a member's integrity (#385): stormraid's counters, the count itself, for that set (and slot) only.
    #[test]
    fn a_sets_integrity_is_its_own_counters() {
        let set = Entity::Raidset { node: "forge1".into(), set: "node-data".into() };
        let slot = Entity::Member { set: "node-data".into(), slot: 3 };
        assert_eq!((set.namespace(), slot.namespace()), (None, None), "a set is in no project");
        for (e, list) in [(&set, RAIDSET), (&slot, MEMBER)] {
            for p in list {
                let q = query(p, Some(e), None).unwrap();
                assert_eq!(p.unit, "count", "{}", p.id);
                assert!(!q.contains("rate("), "a count, not a rate: {q}");
                assert_eq!(q.matches("{job=\"stormraid\",set=\"node-data\"").count(), q.matches("stormraid_").count(), "{q}");
            }
        }
        let q = query(&MEMBER[0], Some(&slot), None).unwrap();
        assert!(q.contains(r#"set="node-data",slot="3"}"#), "{q}");
        let errors = query(RAIDSET.iter().find(|p| p.id == "integrity_errors").unwrap(), Some(&set), None).unwrap();
        for c in ["pi_errors_total", "read_errors_total", "write_errors_total"] {
            assert!(errors.contains(&format!("stormraid_member_{c}")), "{errors}");
        }
        // Every series is one stormraid serves (stormraid `src/http.rs` `integrity_metrics`).
        let served = ["member_checksum_mismatches_total", "member_blocks_repaired_total", "member_read_errors_total",
            "member_write_errors_total", "member_pi_errors_total", "set_unrecoverable_rows_total", "scrub_passes_total"];
        for p in RAIDSET.iter().chain(MEMBER) {
            for name in p.query.split("stormraid_").skip(1).map(|r| r.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).next().unwrap()) {
                assert!(served.contains(&name), "{name} in {}", p.id);
            }
        }
    }

    /// The drive page's Link trend (#339): loss of sync per minute, each side a line.
    #[test]
    fn a_drives_link_is_charted_per_minute_by_side() {
        let e = Entity::Drive { serial: "2EG3K1XJ".into() };
        let p = DRIVE.iter().find(|p| p.id == "link_sync").unwrap();
        let q = query(p, Some(&e), None).unwrap();
        assert!(q.contains(r#"counter="loss_of_sync",serial="2EG3K1XJ""#) && q.ends_with("* 60"), "{q}");
        assert_eq!((p.unit, p.legend), ("per_min", &["side"][..]));
    }

    /// A port's charts (#336): stormport's series for that node and interface only, each counter a line.
    #[test]
    fn a_ports_charts_are_its_own_series() {
        let e = Entity::Port { node: "forge1".into(), ifname: "ens1f0np0".into() };
        assert_eq!(e.namespace(), None, "a port is in no project");
        for p in PORT {
            let q = query(p, Some(&e), None).unwrap();
            assert_eq!(q.matches(r#"job="stormport",node="forge1",ifname="ens1f0np0""#).count(), q.matches("storm_port_").count(), "{q}");
            assert!(!q.contains("__THE__"), "{q}");
        }
        let fec = PORT.iter().find(|p| p.id == "fec_blocks").unwrap();
        let q = query(fec, Some(&e), None).unwrap();
        assert!(q.contains("fec_corrected_blocks_total") && q.contains("fec_uncorrectable_blocks_total"), "{q}");
        assert_eq!(fec.legend, &["counter"]);
        // Every series name is one stormport serves (stormpump docs/ports.md).
        let served = ["link_up", "speed_mbps", "fec_active", "module_temperature_celsius", "module_rx_power_mw", "module_tx_power_mw",
            "rx_packets_total", "tx_packets_total", "rx_bytes_total", "tx_bytes_total", "rx_errors_total", "tx_errors_total",
            "rx_dropped_total", "tx_dropped_total", "rx_crc_errors_total", "rx_missed_errors_total", "fec_corrected_blocks_total",
            "fec_uncorrectable_blocks_total", "pause_rx_frames_total", "pause_tx_frames_total"];
        for p in PORT.iter().chain(LEVELS.iter().flat_map(|l| l.panels).filter(|p| p.job == "stormport")) {
            for name in p.query.split("storm_port_").skip(1).map(|r| r.split(['{', '[']).next().unwrap()) {
                assert!(served.contains(&name), "storm_port_{name} is not a stormport series");
            }
        }
    }

    #[test]
    fn a_watch_is_not_latency_in_either_spelling() {
        let q = panel("apiserver", "latency").query;
        assert!(q.contains("watch") && q.contains("WATCH"), "{q}");
    }

    #[test]
    fn an_object_cannot_end_its_string() {
        let e = Entity::Drive { serial: r#"Z"} or up{a="\"#.into() };
        let q = query(&DRIVE[2], Some(&e), None).unwrap();
        assert!(q.contains(r#"serial="Z\"} or up{a=\"\\"}"#), "{q}");
    }

    #[test]
    fn a_range_answer_becomes_named_lines() {
        let result = serde_json::from_value::<Vec<Value>>(json!([
            {"metric": {"verb": "POST"}, "values": [[100, "1.5"], [130.5, "NaN"]]},
            {"metric": {"verb": "GET"}, "values": [[100, "2"]]},
            {"metric": {}, "values": []}
        ]))
        .unwrap();
        let (l, over) = lines(&result, &["verb"]);
        assert_eq!(over, 0);
        assert_eq!(l.iter().map(|s| s["name"].as_str().unwrap()).collect::<Vec<_>>(), ["GET", "POST", "total"]);
        assert_eq!(l[1]["points"][0], json!({"t": 100000, "v": 1.5}));
        assert_eq!(l[1]["points"][1], json!({"t": 130500, "v": null}), "NaN is a gap");
    }

    #[test]
    fn too_many_lines_are_counted() {
        let result: Vec<Value> = (0..15).map(|i| json!({"metric": {"a": format!("{i:02}")}, "values": []})).collect();
        let (l, over) = lines(&result, &["a"]);
        assert_eq!((l.len(), over), (MAX_SERIES, 3));
    }

    #[test]
    fn windows_and_steps() {
        assert_eq!(clamp_window(None), 3600);
        assert_eq!(clamp_window(Some(1)), 900);
        assert_eq!(clamp_window(Some(i64::MAX)), 90 * 86400);
        assert_eq!(step_for(90 * 86400), 64800, "90 days is still about 120 points");
        assert_eq!(step_for(900), 30);
        assert_eq!(step_for(86400), 720);
    }
}

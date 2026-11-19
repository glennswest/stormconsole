#!/usr/bin/env python3
"""PID 1's side of the host ports, for deploy/verify-node-network.sh (#336).

stormpump's PID 1 is the one writer of every host port (stormcos#958): it reads each port once a second
and publishes /run/stormpump/ports.json, and applies what stormport hands it in ports.d/<ifname>. A build
VM has neither a ConnectX-4 Lx nor PID 1's privileges, so this does that half, in the shapes of stormpump
43b3e23 (`docs/ports.md`, `crates/stormport/src/lib.rs` tests): forge1's two ports, the 25G one down on
baser FEC as it was for two weeks, and a 1G BASE-T one with no module and no FEC. The real stormport reads
this file and keeps the NetworkPort objects.

Rules kept from PID 1:
- a ports.d file is a state: an unknown key or a bad value refuses the whole file (`spec.error`);
- `fec` is applied, after two samples (#235 waits two); every other key is `not_applied`;
- a spec's fec overrides the node's setting (source `spec`); removing the file hands the port back.

    node-network.pid1.py <run dir>
"""
import json
import os
import sys
import time

RUN = sys.argv[1]
PORTS_D = os.path.join(RUN, "ports.d")
os.makedirs(PORTS_D, exist_ok=True)
os.chmod(PORTS_D, 0o700)

FEC = {"rs", "baser", "off", "llrs", "auto"}
KNOWN = {"fec", "autoneg", "speed", "mtu", "pause_rx", "pause_tx", "ring_rx", "ring_tx", "by"}


def now():
    return time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime())


PORTS = {
    "ens1f0np0": {
        "name": "ens1f0np0", "mac": "e8:eb:d3:94:a2:80", "driver": "mlx5_core",
        "driver_version": "6.12.9", "firmware": "14.32.1010 (MT_2420110034)", "bus": "0000:41:00.0",
        "supported_speed": 25000, "speed": None, "link": False, "module_present": True,
        # The node's port-fec.toml leaves it on the driver's default: the switch wants RS.
        "fec_want": None, "fec_source": None, "fec_configured": "baser", "fec_active": "baser",
        "module": {"identifier": 3, "kind": "SFP", "vendor": "Mellanox", "part": "MCP2M00-A002", "serial": "MT2130VS01234",
                   "compliance": ["25GBASE-CR CA-N", "passive copper"], "length_m": 2,
                   "temperature_c": None, "rx_power_mw": None, "tx_power_mw": None},
        "pause": {"rx": True, "tx": True, "rx_frames": 0, "tx_frames": 0},
        "last_pin": None, "spec": None,
        "counters": {"rx_packets": 0, "tx_packets": 0, "rx_bytes": 0, "tx_bytes": 0, "rx_errors": 0, "tx_errors": 0,
                     "rx_dropped": 0, "tx_dropped": 0, "rx_crc_errors": 0, "rx_missed_errors": 0,
                     "fec_corrected_blocks": 0, "fec_uncorrectable_blocks": 0, "pause_rx_frames": 0, "pause_tx_frames": 0},
    },
    "eno1": {
        "name": "eno1", "mac": "b0:26:28:aa:10:01", "driver": "tg3",
        "driver_version": "6.12.9", "firmware": "FFV21.80.8 bc 5720-v1.39", "bus": "0000:01:00.0",
        "supported_speed": 1000, "speed": 1000, "link": True, "module_present": None,
        "fec_want": None, "fec_source": None, "fec_configured": None, "fec_active": None,
        "module": None, "pause": {"rx": False, "tx": False, "rx_frames": 0, "tx_frames": 0},
        "last_pin": None, "spec": None,
        "counters": {"rx_packets": 0, "tx_packets": 0, "rx_bytes": 0, "tx_bytes": 0, "rx_errors": 0, "tx_errors": 0,
                     "rx_dropped": 0, "tx_dropped": 0, "rx_crc_errors": 0, "rx_missed_errors": 0},
    },
}

seen = {}      # ifname -> the file's text last read
pending = {}   # ifname -> (fec, samples left)


def read_spec(ifname):
    path = os.path.join(PORTS_D, ifname)
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        return None
    with os.fdopen(fd) as f:
        return f.read(4096)


def parse(text):
    spec = {"by": None, "fec": None, "not_applied": [], "error": None}
    for line in text.splitlines():
        if not line.strip():
            continue
        k, _, v = line.partition("=")
        if k not in KNOWN and not k.startswith("offload."):
            return {**spec, "error": f"{k}: not a setting"}
        if k == "by":
            spec["by"] = v
        elif k == "fec":
            if v not in FEC:
                return {**spec, "error": f"fec={v}: not rs, baser, off, llrs or auto"}
            spec["fec"] = v
        else:
            if v == "invalid":
                return {**spec, "error": f"{k}={v}: not a value"}
            spec["not_applied"].append(k)
    return spec


def tick(t):
    for ifname, p in PORTS.items():
        text = read_spec(ifname)
        if text != seen.get(ifname):
            seen[ifname] = text
            if text is None:
                p["spec"] = None
                p["fec_want"], p["fec_source"] = None, None
                pending.pop(ifname, None)
            else:
                s = parse(text)
                p["spec"] = s
                print(f"port: {ifname} spec: {text.strip()!r} -> {s}", flush=True)
                if not s["error"] and s["fec"] and p["fec_configured"] is not None:
                    p["fec_want"], p["fec_source"] = s["fec"], "spec"
                    if p["fec_configured"] != s["fec"]:
                        pending[ifname] = (s["fec"], s["by"], 2)
        if ifname in pending:
            fec, by, left = pending[ifname]
            if left > 0:
                pending[ifname] = (fec, by, left - 1)
            else:
                del pending[ifname]
                p["last_pin"] = {"from": p["fec_configured"], "to": fec, "source": "spec", "at": now(), "error": None, "by": by}
                p["fec_configured"] = p["fec_active"] = fec
                print(f"port: {ifname} fec {p['last_pin']['from']} -> {fec}", flush=True)
        # The 25G port: the switch speaks RS, so it has a link on rs and none on anything else.
        if p["fec_configured"] is not None:
            up = p["fec_active"] == "rs"
            p["link"], p["speed"] = up, (25000 if up else None)
            m = p["module"]
            m["temperature_c"] = 33.5 + (t % 10) / 10 if up else None
            c = p["counters"]
            if up:
                c["fec_corrected_blocks"] += 3
                c["rx_packets"] += 900
                c["tx_packets"] += 700
                c["rx_bytes"] += 900 * 1200
                c["tx_bytes"] += 700 * 900
            else:
                c["fec_uncorrectable_blocks"] += 40
                c["rx_errors"] += 40
                c["rx_crc_errors"] += 40
        else:
            c = p["counters"]
            c["rx_packets"] += 50
            c["tx_packets"] += 40
            c["rx_bytes"] += 50 * 600
            c["tx_bytes"] += 40 * 400
    out = {"ports": [{**p, "module_present": p["module_present"]} for p in PORTS.values()]}
    tmp = os.path.join(RUN, "ports.json.tmp")
    with open(tmp, "w") as f:
        json.dump(out, f)
    os.rename(tmp, os.path.join(RUN, "ports.json"))


print(f"pid1 stand-in: {RUN}", flush=True)
t = 0
while True:
    tick(t)
    t += 1
    time.sleep(1)

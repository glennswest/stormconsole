//! No stubs (stormcentral#1455): the build refuses a stub marker in a
//! tracked file, as it refuses warnings. The check is stormcentral's own
//! `deploy/check-stubs.sh`, which the nightly sweep mirrors (#366).

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

#[test]
fn the_checkout_carries_no_stub_marker() {
    let out = Command::new("bash").arg(root().join("deploy/check-stubs.sh")).arg(root()).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn third_party_chunks_are_vendored() {
    // xterm's JoinedCellData carries upstream's own marker for a method the
    // terminal never calls; vite puts a chunk of node_modules code alone
    // under assets/vendor/, which the check (and the sweep) do not read.
    let dist = root().join("web/dist/assets");
    for name in ["xterm.js", "addon-fit.js", "rfb.js"] {
        assert!(dist.join("vendor").join(name).is_file(), "assets/vendor/{name}");
        assert!(!dist.join(name).exists(), "assets/{name} is outside vendor/");
    }
}

#[test]
fn the_check_finds_a_marker() {
    // A negative control: a scratch repo with one marker fails the check.
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-stubs-control");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let git = |args: &[&str]| assert!(Command::new("git").current_dir(&d).args(args).status().unwrap().success());
    git(&["init", "-q"]);
    let marker = ["coming", "soon"].join(" ");
    std::fs::write(d.join("page.js"), format!("label('{marker}')\n")).unwrap();
    git(&["add", "page.js"]);
    let out = Command::new("bash").arg(root().join("deploy/check-stubs.sh")).arg(&d).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("page.js:1:"));
}

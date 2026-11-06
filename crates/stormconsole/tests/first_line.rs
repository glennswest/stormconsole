//! The binary's first line on stderr (#284): stormd keeps what a process
//! wrote, so this line tells a crash in the console from one that never ran it.

use std::process::Command;

fn expected() -> String {
    format!("stormconsole {}: starting, pid ", env!("CARGO_PKG_VERSION"))
}

#[test]
fn the_first_line_comes_first_and_stdout_stays_clean() {
    let out = Command::new(env!("CARGO_BIN_EXE_stormconsole")).arg("--version").output().unwrap();
    assert!(out.status.success());
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.starts_with(&expected()), "{err}");
    // `H=$(… --hash-password)` and the like read stdout alone.
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), format!("stormconsole {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn it_comes_before_a_config_the_console_refuses() {
    let path = std::env::temp_dir().join(format!("first-line-{}.toml", std::process::id()));
    std::fs::write(&path, "no_such_key = 1\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_stormconsole")).arg("--config").arg(&path).output().unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(out.status.code(), Some(78));
    let err = String::from_utf8(out.stderr).unwrap();
    let mut lines = err.lines();
    assert!(lines.next().unwrap().starts_with(&expected()), "{err}");
    assert!(lines.next().unwrap().starts_with("stormconsole: fatal: "), "{err}");
}

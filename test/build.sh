#!/bin/sh
# Build stormconsole's test binary for test/Containerfile, on the build box.
#
# stormcentral runs this first, in the checkout, with cargo and a per-repo
# CARGO_TARGET_DIR (docs/test-standard.md), then `podman build -f
# test/Containerfile <repo root>`. The binary is static (musl) — the target
# the console itself ships as — and is left at test/.stage/stormconsole-test
# for the Containerfile to COPY.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
target=${TARGET:-x86_64-unknown-linux-musl}
cargo build --release --locked --target "$target" --manifest-path "$root/test/Cargo.toml"
tdir=${CARGO_TARGET_DIR:-$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/test/Cargo.toml" |
    sed 's/.*"target_directory":"\([^"]*\)".*/\1/')}
mkdir -p "$root/test/.stage"
cp "$tdir/$target/release/stormconsole-test" "$root/test/.stage/stormconsole-test"
echo "staged $root/test/.stage/stormconsole-test"

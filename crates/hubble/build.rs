//! The protos are compiled by protox, in Rust, so a build needs no `protoc`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");
    let files = ["observer/observer.proto", "relay/relay.proto", "flow/flow.proto"];
    let set = protox::compile(files, ["proto"])?;
    tonic_build::configure().build_client(true).build_server(true).compile_fds(set)?;
    Ok(())
}

//! `/test short|medium|long` — stormconsole's test container
//! (stormcentral `docs/test-standard.md`). Exit 0 all passed, 1 a test
//! failed, 2 the suite could not run.

#[tokio::main]
async fn main() {
    let suite = std::env::args().nth(1);
    let code = stormconsole_test::run(suite).await;
    std::process::exit(code);
}

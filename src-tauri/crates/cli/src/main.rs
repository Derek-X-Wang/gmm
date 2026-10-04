#[tokio::main]
async fn main() {
    let outcome = gmm_cli::run(std::env::args().skip(1).collect()).await;
    println!(
        "{}",
        serde_json::to_string(&outcome).expect("serialize outcome")
    );
    std::process::exit(outcome.exit_code);
}

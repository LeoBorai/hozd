#[tokio::main]
async fn main() {
    if let Err(err) = hozd::run().await {
        eprintln!("hozd: {err:#}");
        std::process::exit(1);
    }
}

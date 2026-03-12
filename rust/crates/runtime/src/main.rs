#[tokio::main]
async fn main() {
    if let Err(error) = sre_runtime::app::run_app().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

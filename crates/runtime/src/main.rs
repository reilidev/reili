#[tokio::main]
async fn main() {
    if let Err(error) = reili_runtime::app::run_app().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

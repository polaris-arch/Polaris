#[cfg(not(debug_assertions))]
compile_error!("native admission verifier is unavailable in release builds");
// Separate executable links the normal production library, never its test build.
#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = if args.len() == 1 && args[0] == "--session" {
        polaris_lib::hold_native_production_verification_session()
    } else if args.len() == 1 {
        polaris_lib::run_native_production_verification(&args[0]).await
    } else {
        Err("exactly one closed native case is required".into())
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

use tracing_subscriber::EnvFilter;

/// Initialise logging. Verbosity is controlled via the `RUST_LOG` environment
/// variable and defaults to `info`.
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

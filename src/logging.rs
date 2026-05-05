use tracing_subscriber::{EnvFilter, fmt};

pub fn init(verbose: bool) {
    let level = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("pip_mirror={level},warn")));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(true)
        .try_init()
        .ok();
}

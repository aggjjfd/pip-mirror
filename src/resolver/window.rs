

/// Re-export. Thin wrapper kept in its own module for testing clarity.
pub use super::pubgrub::compute_version_windows;

// Keep `window.rs` available for direct unit-test access to the core
// algorithm without needing to set up the full resolver chain.

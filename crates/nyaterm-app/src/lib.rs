//! Bundled assets and process diagnostics for the NyaTerm shell.
//!
//! The binary is the real entry point. This library exposes the asset registry
//! for tests and the `icon_gallery` example, and the panic reporter for isolated
//! process tests.

pub mod assets;
pub mod crash_report;

#[cfg(test)]
mod tests {
    #[test]
    fn multilingual_i18n_preload_does_not_use_the_callers_small_stack() {
        let preload = std::thread::Builder::new()
            .name("nyaterm-i18n-small-stack-test".to_string())
            .stack_size(256 * 1024)
            .spawn(nyaterm_desktop::preload_i18n)
            .expect("spawn small-stack preload caller")
            .join()
            .expect("small-stack preload caller panicked");

        assert!(preload.is_ok(), "i18n preload failed: {preload:?}");
        assert!(
            nyaterm_desktop::preload_i18n().is_ok(),
            "i18n preload must be idempotent"
        );
    }
}

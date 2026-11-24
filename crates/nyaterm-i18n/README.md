# nyaterm-i18n

Owns NyaTerm's seven unchanged translation catalogs, legacy locale aliases, stable
picker ordering and one-time preload. It has no workspace or GPUI dependencies.

Call `preload()` before translating on a platform UI thread. The generated
catalog is initialized once on a joined worker with a 16 MiB stack. Repeated
calls return the same result. `apply_locale()` projects the persisted setting
into rust-i18n's process-wide locale; it owns no persisted settings.

Consumers retain ordinary `rust_i18n::t!` calls with an empty local catalog and
`rust_i18n::i18n!("i18n-bridge", fallback = "en", backend = SharedCatalogBackend)`.
The public backend adapter borrows the single catalog. Desktop still configures
component labels and owns language-setting persistence.

`cargo test -p nyaterm-i18n` checks aliases, catalog key/placeholder parity, copy
contracts and cross-crate macro lookup, fallback and interpolation.

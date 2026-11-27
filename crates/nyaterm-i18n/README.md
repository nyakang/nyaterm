# nyaterm-i18n

Owns NyaTerm's seven translation catalogs, legacy locale aliases, stable
picker ordering and one-time preload. It has no workspace or GPUI dependencies.

## Ownership

| Location | Responsibility |
| --- | --- |
| `locales/<locale>.json` | Application copy, grouped by feature namespace; English defines the key and placeholder schema |
| `src/catalog.rs` | Single language registry, incremental catalog tracking, preload and shared backend |
| `src/locale.rs` | Legacy aliases, canonical locale selection and language-picker labels |
| `src/lib.rs` | One rust-i18n catalog compilation and the public facade |
| `src/tests.rs`, `src/tests/catalog.rs` | Catalog structure, copy contracts and locale compatibility |
| `tests/bridge.rs` | Consumer macro lookup, global locale, fallback and interpolation |
| `tests/source_keys.rs` | Literal translation references across all workspace consumers of this crate |

The picker order is English, 简体中文, 繁體中文, 日本語, 한국어, Français,
Português (Brasil). The `catalogs!` invocation in `src/catalog.rs` is the only
language list to maintain; it also generates test fixtures and rustc file tracking.

## Runtime integration

Call `preload()` before translating on a platform UI thread. The generated
catalog is initialized once on a joined worker with a 16 MiB stack. Repeated
calls return the same result. `apply_locale()` projects the persisted setting
into rust-i18n's process-wide locale; it owns no persisted settings.

Consumers retain ordinary `rust_i18n::t!` calls with an empty local catalog and
`rust_i18n::i18n!("i18n-bridge", fallback = "en", backend = SharedCatalogBackend)`.
The public backend adapter borrows the single catalog. Desktop still configures
component labels and owns language-setting persistence.

## Maintaining translations

Add copy to the existing feature namespace in all seven catalogs. Keep keys
stable and use `%{name}` interpolation with identical placeholder names in every
language. `{{name}}` is literal text only in `quickCommands.syntaxHelp`, which
documents command variables. Catalog leaves must be nonblank strings; duplicate
JSON keys, flattened-key collisions and non-string leaves fail validation.

When adding a language, add `locales/<id>.json`, register its id in `catalogs!`,
and define its native `language.name`. Add aliases and compatibility tests in
`src/locale.rs` and `src/tests.rs` as needed. Verify the underlying UI component
library also supports that locale. Stored language settings remain owned by
desktop; existing aliases and English fallback must keep working.

Run `cargo test -p nyaterm-i18n` for catalog validation and shared-backend tests.
This also scans literal `t!("key")` calls in the source trees of every workspace
crate that declares a `nyaterm-i18n` dependency, without building GPUI. It does
not resolve dynamic keys, macro aliases or arbitrary Rust expressions, and is
not an unused-key detector. Dynamic registries need tests in their owning crate;
desktop's shortcut registry is one example. The scan includes source comments,
so translation examples there must also use valid keys.

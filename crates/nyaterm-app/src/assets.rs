use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

/// Bundled UI assets, addressed by the same paths the desktop crate passes to
/// `svg()` and `img()`.
///
/// The directory prefix encodes how an asset is meant to be painted, because the
/// two GPUI elements are not interchangeable:
///
/// * `icons/**` — monochrome. `svg()` rasterizes to an alpha mask and keeps only
///   the coverage channel, so the file's own paints are discarded and the color
///   comes entirely from `.text_color()`.
/// * `color/**` — full color. `img()` keeps the rasterized pixels, and the asset
///   cannot be tinted.
///
/// Invariant: nothing under `icons/` may contain a raster `<image>` payload.
/// GPUI builds resvg without the `raster-images` feature, so embedded bitmaps are
/// skipped silently — the icon renders as nothing at all rather than failing.
/// `color/**` carries real `.png` files instead of base64-in-SVG for that reason.
#[derive(RustEmbed)]
#[folder = "assets/"]
#[include = "icons/**/*.svg"]
#[include = "color/**/*.svg"]
#[include = "color/**/*.png"]
pub struct EmbeddedAssets;

/// Bundled SVG assets for the native shell (activity icons, logo, connection icons).
pub struct NyaTermAssets;

impl AssetSource for NyaTermAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let path = path.trim_start_matches('/');
        if let Some(file) = EmbeddedAssets::get(path) {
            return Ok(Some(file.data));
        }

        // gpui-kit's built-in controls address their bundled icons by
        // names like `icons/search.svg`. Keep NyaTerm assets authoritative, then
        // fall back to the component asset pack for paths we do not ship.
        match gpui_kit::assets::Assets.load(path) {
            Ok(asset) => Ok(asset),
            Err(_) => Ok(None),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let prefix = path.trim_start_matches('/').trim_end_matches('/');
        let mut entries = EmbeddedAssets::iter()
            .filter(|entry| {
                prefix.is_empty()
                    || entry
                        .strip_prefix(prefix)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .map(|entry| SharedString::from(entry.into_owned()))
            .collect::<Vec<_>>();
        if let Ok(component_entries) = gpui_kit::assets::Assets.list(prefix) {
            for entry in component_entries {
                if !entries.contains(&entry) {
                    entries.push(entry);
                }
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use gpui::AssetSource;

    use super::{EmbeddedAssets, NyaTermAssets};

    /// `img()` rasterizes at `SMOOTH_SVG_SCALE_FACTOR` (2.0) times the asset's
    /// *intrinsic* size, not its display size, and `App::fetch_asset` never evicts
    /// the result. A logo left at its authoring size of 256px would hold a 512x512
    /// BGRA buffer (1 MB) to paint a 16px row icon.
    const MAX_COLOR_INTRINSIC_PX: f32 = 64.0;

    fn parse(path: &str, bytes: &[u8]) -> usvg::Tree {
        usvg::Tree::from_data(bytes, &usvg::Options::default())
            .unwrap_or_else(|err| panic!("{path} is not parseable by usvg: {err}"))
    }

    #[test]
    fn every_embedded_svg_parses_with_a_nonzero_size() {
        for path in EmbeddedAssets::iter().filter(|path| path.ends_with(".svg")) {
            let file = EmbeddedAssets::get(&path).expect("iter() yielded a missing asset");
            let size = parse(&path, &file.data).size();
            assert!(
                size.width() > 0.0 && size.height() > 0.0,
                "{path} resolves to a zero size, so it would paint nothing"
            );
        }
    }

    #[test]
    fn no_icon_asset_carries_a_raster_payload() {
        fn find_raster(group: &usvg::Group, path: &str) {
            for node in group.children() {
                match node {
                    usvg::Node::Image(image) => assert!(
                        matches!(image.kind(), usvg::ImageKind::SVG(_)),
                        "{path} embeds a raster image. GPUI builds resvg without the \
                         `raster-images` feature, so this renders blank. Extract the \
                         payload to a real file under color/ and paint it with img()."
                    ),
                    usvg::Node::Group(child) => find_raster(child, path),
                    _ => {}
                }
            }
        }

        for path in EmbeddedAssets::iter().filter(|path| path.starts_with("icons/")) {
            let file = EmbeddedAssets::get(&path).expect("iter() yielded a missing asset");
            find_raster(parse(&path, &file.data).root(), &path);
        }
    }

    #[test]
    fn color_assets_stay_within_the_raster_budget() {
        for path in EmbeddedAssets::iter()
            .filter(|path| path.starts_with("color/") && path.ends_with(".svg"))
        {
            let file = EmbeddedAssets::get(&path).expect("iter() yielded a missing asset");
            let size = parse(&path, &file.data).size();
            let largest = size.width().max(size.height());
            assert!(
                largest <= MAX_COLOR_INTRINSIC_PX,
                "{path} has an intrinsic size of {largest}px; img() would hold a \
                 {}x{} raster forever. Set width/height to {MAX_COLOR_INTRINSIC_PX}px \
                 or less (leave viewBox alone).",
                (largest * 2.0) as u32,
                (largest * 2.0) as u32,
            );
        }
    }

    #[test]
    fn list_returns_only_entries_below_the_requested_prefix() {
        let all = NyaTermAssets.list("").expect("list all");
        let icons = NyaTermAssets.list("icons").expect("list icons");

        assert!(!icons.is_empty(), "no icons are embedded");
        assert!(icons.len() <= all.len());
        assert!(icons.iter().all(|entry| entry.starts_with("icons/")));
    }

    /// Locks the tintable monochrome icons that the SFTP entry context menu and
    /// the remote file preview window address by path. A rename or accidental
    /// deletion of one of these assets would silently paint nothing (the icon is
    /// mask-rendered through `svg()`/`mono_icon`), so pin them here. All live
    /// under `icons/**`, so `no_icon_asset_carries_a_raster_payload` also proves
    /// each stays tintable.
    #[test]
    fn transfer_menu_and_preview_icons_are_embedded() {
        const REQUIRED: &[&str] = &[
            // SFTP entry / current-directory context menu.
            "icons/eye.svg",                 // Preview
            "icons/session/folder-open.svg", // Open
            "icons/copy.svg",                // Copy path / name / dir
            "icons/fe/send-path.svg",        // Send path / name / dir to terminal
            "icons/fe/locate.svg",           // Reveal current directory in tree
            "icons/net/delete.svg",          // Delete (danger)
            // Preview window toolbar.
            "icons/fe/refresh.svg",         // Refresh
            "icons/menu/external.svg",      // Open externally
            "icons/menu/zoom-in.svg",       // Zoom in
            "icons/menu/zoom-out.svg",      // Zoom out
            "icons/menu/fit.svg",           // Reset / fit view
            "icons/menu/rotate-left.svg",   // Rotate left
            "icons/menu/rotate-right.svg",  // Rotate right
            "icons/menu/chevron-left.svg",  // Previous PDF page
            "icons/menu/chevron-right.svg", // Next PDF page
            "icons/file/table.svg",         // Delimited header toggle
        ];
        for path in REQUIRED {
            assert!(
                EmbeddedAssets::get(path).is_some(),
                "{path} is referenced by the SFTP menu or preview window but is not embedded"
            );
        }
    }

    #[test]
    fn notes_panel_icons_are_embedded() {
        const REQUIRED: &[&str] = &[
            "icons/notes.svg",
            "icons/conn/folder.svg",
            "icons/conn/add.svg",
            "icons/fe/new-folder.svg",
            "icons/fe/refresh.svg",
            "icons/session/more.svg",
            "icons/file/description.svg",
        ];
        for path in REQUIRED {
            assert!(
                EmbeddedAssets::get(path).is_some(),
                "{path} is referenced by the Notes panel but is not embedded"
            );
        }
    }
}

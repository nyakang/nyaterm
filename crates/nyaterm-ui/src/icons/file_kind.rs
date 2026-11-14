//! File-explorer entry icons, a port of the old `getFileIcon`.
//!
//! Resolution order is load-bearing: symlink, then directory, then extension,
//! then a dotfile check, then the generic document.

use super::IconDef;
use crate::theme::ThemePalette;

/// Extension groups sharing one icon. Ordered as in the old table so a reviewer
/// can diff the two side by side.
const EXTENSION_ICONS: &[(&[&str], &str, u32)] = &[
    (&["js", "jsx"], "icons/brand/javascript.svg", 0xfacc15),
    (&["ts", "tsx"], "icons/brand/typescript.svg", 0x60a5fa),
    (&["html", "htm"], "icons/brand/html5.svg", 0xf97316),
    (&["css", "scss", "less"], "icons/brand/css.svg", 0x38bdf8),
    (&["py", "pyc"], "icons/brand/python.svg", 0x3776ab),
    (
        &["sh", "bash", "zsh", "bat", "ps1"],
        "icons/conn/terminal.svg",
        0x4ade80,
    ),
    (&["php"], "icons/brand/php.svg", 0x777bb4),
    (
        &["rs", "go", "c", "cpp", "java"],
        "icons/file/code.svg",
        0xf87171,
    ),
    (
        &["json", "yaml", "yml", "toml", "xml"],
        "icons/file/data-object.svg",
        0xa78bfa,
    ),
    (&["sql", "db", "sqlite"], "icons/file/storage.svg", 0x94a3b8),
    (&["doc", "docx"], "icons/file/description.svg", 0x3b82f6),
    (&["pdf"], "icons/file/pdf.svg", 0xef4444),
    (&["xls", "xlsx", "csv"], "icons/file/table.svg", 0x16a34a),
    (&["ppt", "pptx"], "icons/file/present.svg", 0xea580c),
    (
        &["png", "jpg", "jpeg", "gif", "webp", "svg", "ico"],
        "icons/file/image.svg",
        0xec4899,
    ),
    (
        &["mp4", "mkv", "avi", "mov", "webm"],
        "icons/file/movie.svg",
        0x8b5cf6,
    ),
    (
        &["mp3", "wav", "ogg", "flac"],
        "icons/file/audio.svg",
        0xf59e0b,
    ),
    (
        &["zip", "rar", "7z", "tar", "gz", "bz2", "xz"],
        "icons/file/archive.svg",
        0xf59e0b,
    ),
    (
        &["exe", "apk", "dmg", "iso"],
        "icons/file/apps.svg",
        0x14b8a6,
    ),
];

/// Extensions whose icon is themed rather than branded, so they recede instead of
/// competing with the colored ones.
fn muted_extension_icon(extension: &str, palette: ThemePalette) -> Option<IconDef> {
    match extension {
        "ini" | "env" | "conf" | "config" => {
            Some(IconDef::mono("icons/settings.svg", palette.text_muted))
        }
        "md" | "mdx" | "txt" | "rtf" => {
            Some(IconDef::mono("icons/file/text.svg", palette.text_dimmed))
        }
        "lock" => Some(IconDef::mono("icons/lock.svg", palette.text_muted)),
        _ => None,
    }
}

/// Icon for one file-browser row.
pub fn file_entry_icon(
    name: &str,
    is_directory: bool,
    is_symlink: bool,
    palette: ThemePalette,
) -> IconDef {
    if is_symlink {
        return IconDef::mono("icons/conn/symlink.svg", 0x67e8f9);
    }
    if is_directory {
        return IconDef::mono("icons/conn/folder.svg", 0xfbbf24);
    }

    // Only a name that actually contains a dot has an extension. Without this,
    // `Makefile` would "extend" to `makefile` and miss the dotfile branch below.
    if let Some(extension) = name
        .contains('.')
        .then(|| name.rsplit('.').next().unwrap_or_default().to_lowercase())
    {
        if let Some(def) = muted_extension_icon(&extension, palette) {
            return def;
        }
        if let Some((_, path, color)) = EXTENSION_ICONS
            .iter()
            .find(|(extensions, _, _)| extensions.contains(&extension.as_str()))
        {
            return IconDef::mono(path, *color);
        }
    }

    // Dotfiles are configuration by convention, even without a known extension.
    if name.starts_with('.') {
        return IconDef::mono("icons/settings.svg", palette.text_muted);
    }
    IconDef::mono("icons/conn/file.svg", palette.text_muted)
}

#[cfg(test)]
mod tests {
    use super::{IconDef, ThemePalette, file_entry_icon};

    fn palette() -> ThemePalette {
        crate::theme::theme_palette("github-dark")
    }

    fn icon_for(name: &str) -> IconDef {
        file_entry_icon(name, false, false, palette())
    }

    #[test]
    fn directories_and_symlinks_win_over_any_extension() {
        let palette = palette();
        assert_eq!(
            file_entry_icon("archive.zip", true, false, palette).path,
            "icons/conn/folder.svg"
        );
        assert_eq!(
            file_entry_icon("shortcut.png", false, true, palette).path,
            "icons/conn/symlink.svg"
        );
        // A symlinked directory reads as a link, matching the old ordering.
        assert_eq!(
            file_entry_icon("link", true, true, palette).path,
            "icons/conn/symlink.svg"
        );
    }

    #[test]
    fn extensions_are_matched_case_insensitively() {
        assert_eq!(icon_for("Photo.JPEG").path, "icons/file/image.svg");
        assert_eq!(icon_for("Main.RS").path, "icons/file/code.svg");
    }

    #[test]
    fn extensionless_names_do_not_borrow_their_own_name() {
        // `Makefile`.rsplit('.') yields "Makefile", which must not be treated as
        // an extension.
        assert_eq!(icon_for("Makefile").path, "icons/conn/file.svg");
        assert_eq!(icon_for("LICENSE").path, "icons/conn/file.svg");
    }

    #[test]
    fn dotfiles_read_as_configuration() {
        assert_eq!(icon_for(".bashrc").path, "icons/settings.svg");
        assert_eq!(icon_for(".gitignore").path, "icons/settings.svg");
        // A dotfile with a known extension still gets the specific icon.
        assert_eq!(
            icon_for(".eslintrc.json").path,
            "icons/file/data-object.svg"
        );
    }

    #[test]
    fn known_kinds_do_not_collapse_onto_the_generic_document() {
        let generic = icon_for("unknown.qqq").path;
        for name in [
            "a.js", "a.ts", "a.html", "a.css", "a.py", "a.sh", "a.php", "a.rs", "a.json", "a.ini",
            "a.sql", "a.md", "a.doc", "a.pdf", "a.csv", "a.pptx", "a.png", "a.mp4", "a.mp3",
            "a.zip", "a.iso", "a.lock",
        ] {
            assert_ne!(
                icon_for(name).path,
                generic,
                "{name} has no icon of its own"
            );
        }
    }
}

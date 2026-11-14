//! Key normalization for stored connection icons.
//!
//! Icon keys reach us from three places that spell things differently: the app's
//! own picker (canonical), `.nya` backups written by the old app, and session
//! imports from Xshell / MobaXterm / WindTerm (which is where `Ubuntu.svg`,
//! `kali_linux` and `pwsh` come from). Aliasing is deliberately a *read-time*
//! concern — stored values are never rewritten, so a file that round-trips
//! through an older build still resolves.

/// Spelling variants accepted for a canonical icon key. Ported verbatim from the
/// pre-GPUI `CONNECTION_ICON_ALIASES`.
pub(super) const CONNECTION_ICON_ALIASES: &[(&str, &str)] = &[
    ("alibaba-cloudlinux", "alibaba-cloud-linux"),
    ("alibabacloudlinux", "alibaba-cloud-linux"),
    ("alibaba-linux", "alibaba-cloud-linux"),
    ("almalinux", "alma"),
    ("alma-linux", "alma"),
    ("alpine-linux", "alpine"),
    ("alpinelinux", "alpine"),
    ("anolisos", "anolis"),
    ("anolis-os", "anolis"),
    ("archlinux", "arch"),
    ("arch-linux", "arch"),
    ("amazon", "aws"),
    ("amazon-linux", "aws"),
    ("amazonlinux", "aws"),
    ("aws-linux", "aws"),
    ("deepin-a", "deepin"),
    ("command-prompt", "cmd"),
    ("commandprompt", "cmd"),
    ("gitbash", "git"),
    ("git-bash", "git"),
    ("huawei-cloud", "huawei"),
    ("huaweicloud", "huawei"),
    ("kali-linux", "kali"),
    ("kalilinux", "kali"),
    ("linuxmint", "mint"),
    ("linux-mint", "mint"),
    ("nix-os", "nixos"),
    ("open-euler", "openeuler"),
    ("open-suse", "opensuse"),
    ("open-wrt", "openwrt"),
    ("raspberry", "raspberrypi"),
    ("raspberry-pi", "raspberrypi"),
    ("rocky-linux", "rocky"),
    ("rockylinux", "rocky"),
    ("tencent", "tencentos"),
    ("tencent-os", "tencentos"),
    ("tencentlinux", "tencentos"),
    ("ps", "powershell"),
    ("pwsh", "powershell"),
    ("power-shell", "powershell"),
];

/// Fold a stored key toward its canonical spelling: drop a trailing `.svg`,
/// treat `_` as `-`, and lowercase.
pub(super) fn normalize_connection_icon_key(key: &str) -> String {
    let trimmed = key.trim();
    let stripped = trimmed
        .len()
        .checked_sub(4)
        .filter(|split| trimmed[*split..].eq_ignore_ascii_case(".svg"))
        .map_or(trimmed, |split| &trimmed[..split]);
    stripped.replace('_', "-").to_lowercase()
}

pub(super) fn resolve_alias(normalized: &str) -> &str {
    CONNECTION_ICON_ALIASES
        .iter()
        .find(|(alias, _)| *alias == normalized)
        .map_or(normalized, |(_, canonical)| *canonical)
}

#[cfg(test)]
mod tests {
    use super::{CONNECTION_ICON_ALIASES, normalize_connection_icon_key, resolve_alias};

    #[test]
    fn normalization_strips_the_svg_suffix_case_insensitively() {
        assert_eq!(normalize_connection_icon_key("Ubuntu.SVG"), "ubuntu");
        assert_eq!(normalize_connection_icon_key(" Debian.svg "), "debian");
        // A key that merely ends in the same letters must survive intact.
        assert_eq!(normalize_connection_icon_key("nosvg"), "nosvg");
    }

    #[test]
    fn normalization_folds_underscores_and_case() {
        assert_eq!(normalize_connection_icon_key("Kali_Linux"), "kali-linux");
        assert_eq!(normalize_connection_icon_key("ALMA_LINUX"), "alma-linux");
    }

    #[test]
    fn every_alias_points_at_a_different_key() {
        for (alias, canonical) in CONNECTION_ICON_ALIASES {
            assert_ne!(alias, canonical, "{alias} aliases itself");
            assert_eq!(
                *alias,
                normalize_connection_icon_key(alias),
                "{alias} is not in normalized form, so it can never be hit"
            );
        }
    }

    #[test]
    fn aliases_resolve_through_normalization() {
        for (input, expected) in [
            ("kali_linux", "kali"),
            ("Rocky-Linux.svg", "rocky"),
            ("PWSH", "powershell"),
            ("AmazonLinux", "aws"),
        ] {
            assert_eq!(
                resolve_alias(&normalize_connection_icon_key(input)),
                expected
            );
        }
    }
}

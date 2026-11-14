//! Guess a connection's icon from what the remote host reports about itself.
//!
//! Port of the old `inferConnectionIconKeyFromRemoteSystem`. Kept as a pure
//! string function so it can run wherever the stats snapshot arrives, including
//! off the render thread.

/// Distro needles in priority order. First match wins, so the more specific
/// entries have to come before the generic ones — `alibaba cloud linux` must be
/// tested before the bare `linux` fallback at the bottom of [`infer`].
const DISTRO_MATCHES: &[(&[&str], &str)] = &[
    (
        &[
            "alibaba cloud linux",
            "alibaba-cloud-linux",
            "aliyun linux",
            "alinux",
        ],
        "alibaba-cloud-linux",
    ),
    (&["amazon linux", "amzn", "aws linux"], "aws"),
    (&["alma linux", "almalinux"], "alma"),
    (&["alpine"], "alpine"),
    (&["anolis"], "anolis"),
    (&["arch linux", "arch-linux", "archlinux", "arch"], "arch"),
    (&["centos", "cent os"], "centos"),
    (&["debian"], "debian"),
    (&["deepin"], "deepin"),
    (&["fedora"], "fedora"),
    (&["huawei", "opencloudos"], "huawei"),
    (&["kali"], "kali"),
    (&["kylin"], "kylin"),
    (&["linux mint", "linuxmint"], "mint"),
    (&["nixos", "nix os"], "nixos"),
    (&["open euler", "openeuler"], "openeuler"),
    (&["openwrt", "open-wrt"], "openwrt"),
    (&["opensuse", "open suse", "sles", "suse"], "opensuse"),
    (&["rocky"], "rocky"),
    (&["tencent", "tlinux"], "tencentos"),
    (&["ubuntu"], "ubuntu"),
    (&["uniontech", "uos"], "uos"),
];

fn normalize(os: &str, arch: &str) -> String {
    let joined = format!("{os} {arch}");
    let mut out = String::with_capacity(joined.len());
    let mut pending_space = false;
    for ch in joined.chars() {
        let ch = match ch {
            '_' | '.' | '/' => '-',
            _ => ch,
        };
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// Whole-word match, so `arch` does not fire on `search` or `aarch64`.
fn has_token(text: &str, token: &str) -> bool {
    let is_word_char = |ch: char| ch.is_ascii_alphanumeric();
    text.match_indices(token).any(|(start, matched)| {
        let before_ok = text[..start]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_word_char(ch));
        let after_ok = text[start + matched.len()..]
            .chars()
            .next()
            .is_none_or(|ch| !is_word_char(ch));
        before_ok && after_ok
    })
}

/// Alphanumeric needles are token-matched; anything containing a separator is a
/// plain substring test. This is per-needle, not per-group, so `alinux` is
/// token-matched while its sibling `alibaba cloud linux` is not.
fn matches_needle(text: &str, needle: &str) -> bool {
    if needle.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        has_token(text, needle)
    } else {
        text.contains(needle)
    }
}

/// Map a remote host's reported OS/arch to a connection icon key.
pub fn infer_connection_icon_key_from_remote_system(os: &str, arch: &str) -> Option<&'static str> {
    let text = normalize(os, arch);
    if text.is_empty() {
        return None;
    }

    for (needles, icon_key) in DISTRO_MATCHES {
        if needles.iter().any(|needle| matches_needle(&text, needle)) {
            return Some(icon_key);
        }
    }

    if text.contains("windows") || text.contains("mingw") || text.contains("msys") {
        return Some("windows");
    }
    if text.contains("darwin")
        || text.contains("macos")
        || text.contains("mac os")
        || text.contains("os x")
    {
        return Some("apple");
    }
    if text.contains("linux") || text.contains("gnu") {
        return Some("linux");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::connection::resolve_connection_icon;
    use super::super::{IconPaint, connection::default_connection_icon_for_kind};
    use super::{DISTRO_MATCHES, infer_connection_icon_key_from_remote_system};

    fn infer(os: &str) -> Option<&'static str> {
        infer_connection_icon_key_from_remote_system(os, "x86_64")
    }

    #[test]
    fn common_uname_strings_land_on_the_right_distro() {
        assert_eq!(infer("Ubuntu 24.04.1 LTS"), Some("ubuntu"));
        assert_eq!(infer("CentOS Linux 7"), Some("centos"));
        assert_eq!(infer("Debian GNU/Linux 12"), Some("debian"));
        assert_eq!(infer("Rocky Linux 9.4"), Some("rocky"));
        assert_eq!(infer("openEuler 24.03"), Some("openeuler"));
        assert_eq!(infer("OpenWrt 24.10.0"), Some("openwrt"));
        assert_eq!(infer("Alibaba Cloud Linux 3"), Some("alibaba-cloud-linux"));
        assert_eq!(infer("Amazon Linux 2023"), Some("aws"));
        assert_eq!(infer("Kylin V10"), Some("kylin"));
        assert_eq!(infer("UnionTech OS Server 20"), Some("uos"));
    }

    #[test]
    fn specific_distros_beat_the_generic_linux_fallback() {
        // Every one of these also contains "linux".
        assert_eq!(infer("Alpine Linux v3.20"), Some("alpine"));
        assert_eq!(infer("Kali GNU/Linux Rolling"), Some("kali"));
        assert_eq!(infer("Linux Mint 22"), Some("mint"));
    }

    #[test]
    fn unrecognized_unix_still_reports_something_useful() {
        assert_eq!(infer("Linux 6.8.0-generic"), Some("linux"));
        assert_eq!(infer("Darwin"), Some("apple"));
        assert_eq!(infer("MINGW64_NT-10.0"), Some("windows"));
    }

    #[test]
    fn arch_matching_respects_word_boundaries() {
        // `aarch64` must not read as Arch Linux.
        assert_eq!(
            infer_connection_icon_key_from_remote_system("Ubuntu 24.04", "aarch64"),
            Some("ubuntu")
        );
        assert_eq!(
            infer_connection_icon_key_from_remote_system("Linux", "aarch64"),
            Some("linux")
        );
        assert_eq!(infer("Arch Linux"), Some("arch"));
    }

    #[test]
    fn empty_input_infers_nothing() {
        assert_eq!(infer_connection_icon_key_from_remote_system("", ""), None);
        assert_eq!(
            infer_connection_icon_key_from_remote_system("   ", " "),
            None
        );
        assert_eq!(infer("Plan 9"), None);
    }

    #[test]
    fn every_inferred_key_resolves_to_a_real_icon() {
        let generic = default_connection_icon_for_kind("SSH");
        for (_, icon_key) in DISTRO_MATCHES {
            let def = resolve_connection_icon(Some(icon_key), "SSH");
            assert_ne!(def, generic, "inferred key {icon_key} has no icon");
            assert_eq!(
                def.paint,
                IconPaint::FullColor,
                "inferred key {icon_key} should land on an OS logo"
            );
        }
    }
}

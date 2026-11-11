//! Connection icon tables, ported from the pre-GPUI `icons.tsx`.
//!
//! Four tables are merged into one lookup, in the same precedence the old app
//! used: server → linux → brand → system, later winning. That ordering is
//! load-bearing, not incidental: eight keys (`apple`, `linux`, `ubuntu`,
//! `debian`, `centos`, `fedora`, `k8s`, `aws`) appear in both the brand table and
//! the system table, and connections must get the full-color OS logo rather than
//! the monochrome brand mark.

use super::IconDef;
use super::aliases::{normalize_connection_icon_key, resolve_alias};

/// Blue server key retained for existing explicit selections and batch editing.
pub(in crate::features) const DEFAULT_CONNECTION_ICON: &str = "server";

/// The default glyph in seven theme-friendly hues. One asset, seven tints — this
/// is exactly what a monochrome alpha mask is for.
const SERVER_ICONS: &[(&str, IconDef)] = &[
    ("server", IconDef::mono("icons/brand/server.svg", 0x60a5fa)),
    (
        "server-emerald",
        IconDef::mono("icons/brand/server.svg", 0x34d399),
    ),
    (
        "server-amber",
        IconDef::mono("icons/brand/server.svg", 0xfbbf24),
    ),
    (
        "server-rose",
        IconDef::mono("icons/brand/server.svg", 0xfb7185),
    ),
    (
        "server-violet",
        IconDef::mono("icons/brand/server.svg", 0xa78bfa),
    ),
    (
        "server-cyan",
        IconDef::mono("icons/brand/server.svg", 0x22d3ee),
    ),
    (
        "server-slate",
        IconDef::mono("icons/brand/server.svg", 0x94a3b8),
    ),
];

/// The same seven hues on Tux, for people who want a Linux mark without
/// committing to a distro.
const LINUX_ICONS: &[(&str, IconDef)] = &[
    (
        "linux-default",
        IconDef::mono("icons/brand/linux.svg", 0x60a5fa),
    ),
    (
        "linux-emerald",
        IconDef::mono("icons/brand/linux.svg", 0x34d399),
    ),
    (
        "linux-amber",
        IconDef::mono("icons/brand/linux.svg", 0xfbbf24),
    ),
    (
        "linux-rose",
        IconDef::mono("icons/brand/linux.svg", 0xfb7185),
    ),
    (
        "linux-violet",
        IconDef::mono("icons/brand/linux.svg", 0xa78bfa),
    ),
    (
        "linux-cyan",
        IconDef::mono("icons/brand/linux.svg", 0x22d3ee),
    ),
    (
        "linux-slate",
        IconDef::mono("icons/brand/linux.svg", 0x94a3b8),
    ),
];

/// Service and language marks. Colors are the vendors' published brand values;
/// several are near-black and get lifted at paint time by [`IconDef::tint`].
pub(super) const BRAND_ICONS: &[(&str, IconDef)] = &[
    ("docker", IconDef::mono("icons/brand/docker.svg", 0x2496ed)),
    ("k8s", IconDef::mono("icons/brand/kubernetes.svg", 0x326ce5)),
    ("linux", IconDef::mono("icons/brand/linux.svg", 0xfcc624)),
    ("ubuntu", IconDef::mono("icons/brand/ubuntu.svg", 0xe95420)),
    ("debian", IconDef::mono("icons/brand/debian.svg", 0xa81d33)),
    ("centos", IconDef::mono("icons/brand/centos.svg", 0x262577)),
    ("fedora", IconDef::mono("icons/brand/fedora.svg", 0x3c4fb1)),
    ("apple", IconDef::mono("icons/brand/apple.svg", 0xa2aaad)),
    ("github", IconDef::mono("icons/brand/github.svg", 0x181717)),
    ("gitlab", IconDef::mono("icons/brand/gitlab.svg", 0xfc6d26)),
    ("nginx", IconDef::mono("icons/brand/nginx.svg", 0x009639)),
    ("redis", IconDef::mono("icons/brand/redis.svg", 0xdc382d)),
    (
        "postgres",
        IconDef::mono("icons/brand/postgresql.svg", 0x4169e1),
    ),
    ("mysql", IconDef::mono("icons/brand/mysql.svg", 0x4479a1)),
    (
        "mongodb",
        IconDef::mono("icons/brand/mongodb.svg", 0x47a248),
    ),
    ("python", IconDef::mono("icons/brand/python.svg", 0x3776ab)),
    ("js", IconDef::mono("icons/brand/javascript.svg", 0xf7df1e)),
    ("ts", IconDef::mono("icons/brand/typescript.svg", 0x3178c6)),
    ("rust", IconDef::mono("icons/brand/rust.svg", 0x000000)),
    ("go", IconDef::mono("icons/brand/go.svg", 0x00add8)),
    ("node", IconDef::mono("icons/brand/nodedotjs.svg", 0x339933)),
    ("php", IconDef::mono("icons/brand/php.svg", 0x777bb4)),
    ("aws", IconDef::mono("icons/brand/aws.svg", 0x232f3e)),
    (
        "gcp",
        IconDef::mono("icons/brand/googlecloud.svg", 0x4285f4),
    ),
];

/// Official OS and distro logos, kept in full color. These override the brand
/// table on the eight colliding keys.
const SYSTEM_ICONS: &[(&str, IconDef)] = &[
    ("windows", IconDef::full_color("color/os/windows.svg")),
    ("apple", IconDef::full_color("color/os/apple.svg")),
    ("android", IconDef::full_color("color/os/android.svg")),
    ("linux", IconDef::full_color("color/os/linux.svg")),
    ("ubuntu", IconDef::full_color("color/os/ubuntu.svg")),
    ("debian", IconDef::full_color("color/os/debian.svg")),
    ("centos", IconDef::full_color("color/os/centos.svg")),
    ("fedora", IconDef::full_color("color/os/fedora.svg")),
    ("arch", IconDef::full_color("color/os/archlinux.svg")),
    ("manjaro", IconDef::full_color("color/os/manjaro.svg")),
    ("opensuse", IconDef::full_color("color/os/opensuse.svg")),
    ("rocky", IconDef::full_color("color/os/rocky-linux.svg")),
    ("alma", IconDef::full_color("color/os/almalinux.svg")),
    ("alpine", IconDef::full_color("color/os/alpine-linux.svg")),
    ("kali", IconDef::full_color("color/os/kalilinux.svg")),
    ("mint", IconDef::full_color("color/os/linux-mint.svg")),
    ("nixos", IconDef::full_color("color/os/nixos.svg")),
    ("h3c", IconDef::full_color("color/os/h3c.svg")),
    ("k8s", IconDef::full_color("color/os/k8s.svg")),
    ("gentoo", IconDef::full_color("color/os/gentoo.svg")),
    (
        "raspberrypi",
        IconDef::full_color("color/os/raspberrypi.svg"),
    ),
    ("openwrt", IconDef::full_color("color/os/openwrt.svg")),
    (
        "alibaba-cloud-linux",
        IconDef::full_color("color/os/alibabacloudlinux.svg"),
    ),
    ("anolis", IconDef::full_color("color/os/anolisos.svg")),
    ("deepin", IconDef::full_color("color/os/deepin.svg")),
    ("kylin", IconDef::full_color("color/os/kylin.svg")),
    ("openeuler", IconDef::full_color("color/os/openeuler.svg")),
    ("tencentos", IconDef::full_color("color/os/tencentos.svg")),
    ("uos", IconDef::full_color("color/os/uos.svg")),
    ("aws", IconDef::full_color("color/os/aws.svg")),
    ("huawei", IconDef::full_color("color/os/huawei.svg")),
    ("git", IconDef::full_color("color/os/git.svg")),
    ("cmd", IconDef::full_color("color/os/cmd.svg")),
    ("powershell", IconDef::full_color("color/os/powershell.svg")),
];

/// Protocol defaults are selectable with the same assets and tints used when a
/// connection has no explicit icon.
const PROTOCOL_ICONS: &[(&str, IconDef)] = &[
    ("local", IconDef::mono("icons/conn/terminal.svg", 0x4ade80)),
    ("telnet", IconDef::mono("icons/conn/telnet.svg", 0xd29922)),
    ("serial", IconDef::mono("icons/conn/serial.svg", 0xbc8cff)),
    ("rdp", IconDef::mono("icons/conn/rdp.svg", 0x22d3ee)),
    ("vnc", IconDef::mono("icons/conn/vnc.svg", 0xfb7185)),
];

/// Keys the GPUI build has written that the old app never had. They are not
/// offered in the picker, but stored connections already carry them, so they must
/// keep resolving. Do not prune this list without a data migration.
const LEGACY_ICONS: &[(&str, IconDef)] = &[
    ("kubernetes", IconDef::full_color("color/os/k8s.svg")),
    (
        "terminal",
        IconDef::mono("icons/conn/terminal.svg", 0x4ade80),
    ),
    ("folder", IconDef::mono("icons/conn/folder.svg", 0xfbbf24)),
    ("group", IconDef::mono("icons/conn/folder.svg", 0xfbbf24)),
    // The old app had no FreeBSD entry; the GPUI picker offered one backed by the
    // generic Tux glyph. Keep it resolving for anyone who picked it.
    ("freebsd", IconDef::mono("icons/brand/linux.svg", 0xab2b28)),
];

/// Picker contents, in the old app's display order: the seven server hues, the
/// seven Linux hues, then every OS logo, followed by protocol defaults.
pub(in crate::features) fn connection_icon_options() -> impl Iterator<Item = &'static str> {
    SERVER_ICONS
        .iter()
        .chain(LINUX_ICONS)
        .chain(SYSTEM_ICONS)
        .chain(PROTOCOL_ICONS)
        .map(|(key, _)| *key)
}

/// Materialized picker list, so grid code can index and count without collecting.
pub(in crate::features) static CONNECTION_ICON_OPTIONS: std::sync::LazyLock<Vec<&'static str>> =
    std::sync::LazyLock::new(|| connection_icon_options().collect());

fn lookup(key: &str) -> Option<IconDef> {
    // Reverse precedence order: the table that wins is consulted first.
    for table in [
        SYSTEM_ICONS,
        BRAND_ICONS,
        LINUX_ICONS,
        SERVER_ICONS,
        PROTOCOL_ICONS,
        LEGACY_ICONS,
    ] {
        if let Some((_, def)) = table.iter().find(|(candidate, _)| *candidate == key) {
            return Some(*def);
        }
    }
    None
}

/// Resolve a stored icon key, falling back to a glyph for the connection kind.
///
/// Matches the old `resolveConnectionIcon` exactly: an exact key wins first, then
/// the normalized-and-aliased form, then the default. Step one is byte-exact on
/// purpose — normalizing up front would let `Ubuntu` and `ubuntu` diverge if a
/// case-sensitive key is ever added.
pub(in crate::features) fn resolve_connection_icon(icon_key: Option<&str>, kind: &str) -> IconDef {
    if let Some(key) = icon_key.filter(|key| !key.is_empty()) {
        if let Some(def) = lookup(key) {
            return def;
        }
        let normalized = normalize_connection_icon_key(key);
        if let Some(def) = lookup(resolve_alias(&normalized)) {
            return def;
        }
    }
    default_connection_icon_for_kind(kind)
}

/// Kind-specific default. The old app always fell back to the blue server glyph;
/// giving each non-SSH kind its own mark also distinguishes saved connections
/// and session tabs when no custom icon is selected.
pub(in crate::features) fn default_connection_icon_for_kind(kind: &str) -> IconDef {
    let key = match kind {
        "Local" => "local",
        "Telnet" => "telnet",
        "Serial" => "serial",
        "RDP" => "rdp",
        "VNC" => "vnc",
        "ADB" | "Android" | "Android (ADB)" => "android",
        _ => DEFAULT_CONNECTION_ICON,
    };
    lookup(key).expect("protocol defaults must be registered connection icons")
}

#[cfg(test)]
mod tests {
    use crate::models::ConnectionKindTab;
    use nyaterm_core::models::sessions::SavedConnection;

    use super::super::IconPaint;
    use super::super::aliases::CONNECTION_ICON_ALIASES;
    use super::{
        CONNECTION_ICON_OPTIONS, LINUX_ICONS, PROTOCOL_ICONS, SERVER_ICONS, SYSTEM_ICONS,
        default_connection_icon_for_kind, lookup, resolve_connection_icon,
    };

    /// Every key the GPUI picker has ever offered. Stored connections carry these
    /// verbatim, so none may stop resolving.
    const PREVIOUSLY_OFFERED_KEYS: &[&str] = &[
        "server",
        "server-emerald",
        "server-amber",
        "server-rose",
        "server-violet",
        "server-cyan",
        "server-slate",
        "windows",
        "apple",
        "android",
        "linux",
        "ubuntu",
        "debian",
        "centos",
        "fedora",
        "arch",
        "manjaro",
        "opensuse",
        "rocky",
        "alma",
        "alpine",
        "kali",
        "mint",
        "nixos",
        "gentoo",
        "freebsd",
        "raspberrypi",
        // Written by code paths rather than the picker.
        "local",
        "terminal",
        "telnet",
        "serial",
        "folder",
        "group",
        "docker",
        "python",
        "github",
        "kubernetes",
        "k8s",
        "nginx",
        "redis",
        "postgres",
        "mysql",
        "mongodb",
        "js",
        "ts",
        "rust",
        "go",
        "node",
        "php",
        "aws",
        "gcp",
        "gitlab",
    ];

    fn resolves(key: &str) -> bool {
        resolve_connection_icon(Some(key), "SSH") != default_connection_icon_for_kind("SSH")
            || key == "server"
    }

    #[test]
    fn every_previously_stored_key_still_resolves() {
        for key in PREVIOUSLY_OFFERED_KEYS {
            assert!(
                resolves(key),
                "{key} fell through to the default icon; stored connections would \
                 silently lose their icon"
            );
        }
    }

    #[test]
    fn every_alias_resolves_to_a_real_icon() {
        for (alias, canonical) in CONNECTION_ICON_ALIASES {
            assert!(
                lookup(canonical).is_some(),
                "alias {alias} points at {canonical}, which is not in any table"
            );
            assert_eq!(
                resolve_connection_icon(Some(alias), "SSH"),
                resolve_connection_icon(Some(canonical), "SSH"),
            );
        }
    }

    #[test]
    fn system_logos_win_over_brand_marks_on_shared_keys() {
        for key in [
            "apple", "linux", "ubuntu", "debian", "centos", "fedora", "k8s", "aws",
        ] {
            let def = resolve_connection_icon(Some(key), "SSH");
            assert_eq!(
                def.paint,
                IconPaint::FullColor,
                "{key} must use the full-color OS logo, not the brand mark"
            );
        }
    }

    #[test]
    fn distros_no_longer_share_one_glyph() {
        // The whole point of the change: these all pointed at conn/linux.svg and
        // differed only by tint.
        let distros = [
            "centos",
            "fedora",
            "arch",
            "manjaro",
            "opensuse",
            "rocky",
            "alma",
            "alpine",
            "kali",
            "mint",
            "nixos",
            "gentoo",
            "raspberrypi",
        ];
        let mut paths: Vec<_> = distros
            .iter()
            .map(|key| resolve_connection_icon(Some(key), "SSH").path)
            .collect();
        paths.sort_unstable();
        let total = paths.len();
        paths.dedup();
        assert_eq!(paths.len(), total, "some distros still share an asset");
    }

    #[test]
    fn kubernetes_is_not_a_whale() {
        let k8s = resolve_connection_icon(Some("k8s"), "SSH");
        let docker = resolve_connection_icon(Some("docker"), "SSH");
        assert_ne!(k8s.path, docker.path);
    }

    #[test]
    fn unknown_and_empty_keys_fall_back_by_kind() {
        let mut paths = Vec::new();
        for kind in ["SSH", "Local", "Telnet", "Serial", "RDP", "VNC", "ADB"] {
            let default = default_connection_icon_for_kind(kind);
            for key in [None, Some(""), Some("   "), Some("nonsense")] {
                assert_eq!(resolve_connection_icon(key, kind), default);
            }
            // Switching protocols must preserve an explicitly chosen icon.
            assert_eq!(
                resolve_connection_icon(Some("ubuntu"), kind),
                resolve_connection_icon(Some("ubuntu"), "SSH"),
            );
            assert!(
                !paths.contains(&default.path),
                "{kind} shares its default glyph with another session kind"
            );
            paths.push(default.path);
        }
    }

    #[test]
    fn saved_android_connection_keeps_the_editor_default_icon() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "name": "android-1",
            "type": "adb",
            "device": { "kind": "usb", "serial": "test-device" }
        }))
        .expect("saved Android connection must load without an explicit icon");
        let android = lookup("android").expect("Android logo must be registered");
        for kind in [
            connection.kind_label(),
            ConnectionKindTab::Adb.label(),
            "Android",
        ] {
            assert_eq!(
                resolve_connection_icon(connection.icon.as_deref(), kind),
                android
            );
            assert_eq!(
                resolve_connection_icon(Some("server"), kind),
                lookup("server").unwrap(),
                "an explicitly selected icon must still take precedence"
            );
        }
        assert_eq!(android.paint, IconPaint::FullColor);
    }

    #[test]
    fn picker_lists_every_hue_and_every_logo_without_duplicates() {
        let options = &*CONNECTION_ICON_OPTIONS;
        assert_eq!(
            options.len(),
            SERVER_ICONS.len() + LINUX_ICONS.len() + SYSTEM_ICONS.len() + PROTOCOL_ICONS.len()
        );
        let mut sorted = options.clone();
        sorted.sort_unstable();
        let total = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), total, "the picker offers a key twice");

        for key in options {
            assert!(
                lookup(key).is_some(),
                "picker offers unresolvable key {key}"
            );
        }
    }

    #[test]
    fn every_protocol_default_has_one_matching_picker_option() {
        for kind in ["SSH", "Local", "Telnet", "Serial", "RDP", "VNC", "ADB"] {
            let default = resolve_connection_icon(None, kind);
            let matching_options = CONNECTION_ICON_OPTIONS
                .iter()
                .filter(|key| resolve_connection_icon(Some(key), kind) == default)
                .count();
            assert_eq!(matching_options, 1, "{kind} default must be selectable");
        }
        for key in ["rdp", "vnc"] {
            assert_eq!(
                resolve_connection_icon(Some(key), "SSH"),
                resolve_connection_icon(None, &key.to_uppercase()),
                "explicit protocol icons must survive switching protocols"
            );
        }
    }
}

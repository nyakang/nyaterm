use super::{LineRole, MAX_LINE_ROLE_BYTES, detect, fields};

#[test]
fn indented_listing_fields_use_original_utf8_offsets() {
    for indent in ["", "  ", "\t", "\u{2003}".repeat(4).as_str(), "\u{3000}\t "] {
        for marker in ["", ".", "+", "@"] {
            let line = format!(
                "{indent}-rw-r--r--{marker}\t1\troot\tstaff\t123\tOct 30 10:00 中文e\u{301}.txt"
            );
            assert_eq!(detect(&line), LineRole::FileListingOutput);
            let parsed = fields(detect(&line), &line);
            for (token, rule) in [
                ("1", "builtin-number"),
                ("123", "builtin-number"),
                ("Oct", "builtin-month"),
            ] {
                assert!(
                    parsed
                        .iter()
                        .any(|field| field.rule == rule && &line[field.range.clone()] == token),
                    "{line:?}: {parsed:?}"
                );
            }
            assert!(parsed.iter().all(|field| field.range.start >= indent.len()));
        }
    }
}

#[test]
fn every_detected_role_keeps_ranges_on_utf8_boundaries_under_mutation() {
    let fixtures = [
        "2: eth0: <UP,BROADCAST> mtu 1500",
        "inet 10.0.0.1/24 scope global eth0",
        "-rw-r--r-- 1 root root 123 Oct 30 10:00 中文.txt",
        "root 1234 0.0 0.1 123456 7890 ? Ss 10:00 0:00 /usr/sbin/sshd -D",
        "root 1234 1 0 Oct30 ? 00:00:01 /usr/sbin/sshd -D",
        "diff --git a/中文.txt b/中文.txt",
        "Author: 姓名 <a@example.com>",
        "  File: 中文.txt",
        "Access: (0644/-rw-r--r--) Uid: (0/root)",
        "user::rwx",
        "test 中文 ... ok",
        "   Compiling serde v1.0.0",
        "main.c:10:5: error: message",
        "SELECT name FROM users WHERE id = 123;",
        "On branch main",
        "\tmodified: 中文.txt",
        "123456789abc nginx latest Up 2 hours 0.0.0.0:80",
        " Active: active (running) since Fri 10:00",
        "tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:*",
        "/dev/sda1 100G 50G 50G 50% /",
        "Mem: 31Gi 5Gi 20Gi 1Gi 6Gi 25Gi",
        "error[E0308]: mismatched types",
        "2026/10/30 21:00:00 [error] 123#0: message",
        "2026-10-30 13:15:13 INFO 中文\u{2003}src/main.rs:123 message",
        "[GIN] 2026/10/30 - 23:14:08 | 200 | GET /health",
        "interface GigabitEthernet1/0/1",
        " ip address 192.168.1.1 255.255.255.0",
    ];
    for seed in fixtures {
        for prefix in ["", "  ", "\t", "\u{2003}\u{2003}\u{2003}\u{2003}"] {
            let source = format!("{prefix}{seed}");
            let boundaries: Vec<_> = source
                .char_indices()
                .map(|(at, _)| at)
                .chain(std::iter::once(source.len()))
                .collect();
            for at in boundaries {
                for inserted in ["", "中", "\u{2003}", "\u{301}", "🚀"] {
                    let mut line = source.clone();
                    line.insert_str(at, inserted);
                    for field in fields(detect(&line), &line) {
                        assert!(
                            !field.range.is_empty() && line.get(field.range.clone()).is_some(),
                            "{line:?}: {field:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn specialized_roles_stop_at_the_byte_limit_without_truncating_text() {
    let mut line = "-rw-r--r-- 1 root root 123 Oct 30 10:00 ".to_string();
    line.extend(std::iter::repeat_n('a', MAX_LINE_ROLE_BYTES - line.len()));
    assert_eq!(detect(&line), LineRole::FileListingOutput);
    line.push('中');
    assert_eq!(detect(&line), LineRole::Unknown);
    assert!(fields(LineRole::FileListingOutput, &line).is_empty());
}

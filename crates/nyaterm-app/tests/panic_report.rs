#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::panic;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use nyaterm_app::crash_report::install;
use nyaterm_core::test_support::TestTempDir;

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--panic-report-child")) {
        let scenario = args.next().expect("child scenario");
        let root = args.next().expect("child directory");
        run_child(scenario.to_str().unwrap(), Path::new(&root));
        return;
    }

    for (scenario, reports, hook_calls, success) in [
        ("normal", 0, 0, true),
        ("uncaught", 1, 1, false),
        ("caught", 1, 1, true),
        ("background", 1, 1, true),
        ("concurrent", 8, 8, true),
        ("quota", 20, 25, true),
        ("blocked", 0, 1, true),
        ("removed", 0, 1, true),
        ("failed-quota", 0, 21, true),
        ("long", 1, 1, true),
        ("non-string", 1, 1, true),
    ] {
        let root = TestTempDir::new("nyaterm-panic-process");
        fs::create_dir_all(root.path()).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--panic-report-child")
            .arg(scenario)
            .arg(root.path())
            .env_remove("RUST_BACKTRACE")
            .env_remove("RUST_LIB_BACKTRACE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("panic reporter child timed out: {scenario}");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(
            status.success(),
            success,
            "child status: {scenario}: {status}"
        );
        let calls = fs::read(root.join("previous-hook")).unwrap_or_default();
        assert_eq!(calls, vec![b'p'; hook_calls], "previous hook: {scenario}");
        let paths: Vec<_> = if root.join("crashes").is_dir() {
            fs::read_dir(root.join("crashes"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect()
        } else {
            Vec::new()
        };
        assert_eq!(paths.len(), reports, "report count: {scenario}");
        for path in paths {
            let bytes = fs::read(&path).unwrap();
            assert!(bytes.len() <= 256 * 1024, "report size: {scenario}");
            let report = String::from_utf8(bytes).expect("valid UTF-8 report");
            for field in [
                "Format-Version: 1",
                "Event: rust_panic",
                "Timestamp-UTC-Unix-Ms:",
                "Version:",
                "OS:",
                "Arch:",
                "PID:",
                "Thread-ID:",
                "Location:",
                "Thread-Name:",
                "Message:",
            ] {
                assert!(report.contains(field), "missing {field}: {scenario}");
            }
            if scenario == "long" {
                assert!(report.ends_with("\n[report truncated]\n"));
                assert!(report.contains("猫🐈"));
            } else {
                assert!(report.contains("Backtrace:\n"), "backtrace: {scenario}");
                assert!(!report.contains("disabled backtrace"));
            }
            if scenario == "uncaught" {
                assert!(report.contains("uncaught main panic fixture"));
                assert!(report.contains("panic_report.rs:"));
            }
            if matches!(scenario, "background" | "concurrent") {
                assert!(report.contains("Thread-Name: panic-worker-"));
            }
            if scenario == "non-string" {
                assert!(report.contains("<non-string panic payload>"));
            }
        }
        println!("panic reporter scenario passed: {scenario}");
    }
}

fn run_child(scenario: &str, root: &Path) {
    // A known previous hook proves chaining without noisy default backtraces or
    // console handles. The marker also survives an uncaught main-thread panic.
    let marker = root.join("previous-hook");
    panic::set_hook(Box::new(move |_| {
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&marker)
            .and_then(|mut file| file.write_all(b"p"));
    }));
    let crash_dir = root.join("crashes");
    if scenario == "blocked" {
        fs::write(&crash_dir, b"blocked").unwrap();
        assert!(install(crash_dir).is_err());
        assert!(panic::catch_unwind(|| panic!("directory unavailable")).is_err());
        return;
    }
    install(crash_dir.clone()).unwrap();
    match scenario {
        "normal" => {}
        "uncaught" => panic!("uncaught main panic fixture"),
        "caught" => {
            assert!(panic::catch_unwind(|| panic!("caught panic fixture")).is_err());
        }
        "background" | "concurrent" => {
            let count = if scenario == "concurrent" { 8 } else { 1 };
            let barrier = Arc::new(Barrier::new(count));
            let threads: Vec<_> = (0..count)
                .map(|index| {
                    let barrier = barrier.clone();
                    std::thread::Builder::new()
                        .name(format!("panic-worker-{index}"))
                        .spawn(move || {
                            barrier.wait();
                            panic!("worker panic fixture {index}");
                        })
                        .unwrap()
                })
                .collect();
            for thread in threads {
                assert!(thread.join().is_err());
            }
        }
        "quota" => {
            for index in 0..25 {
                assert!(panic::catch_unwind(|| panic!("quota fixture {index}")).is_err());
            }
        }
        "removed" => {
            fs::remove_dir(&crash_dir).unwrap();
            assert!(panic::catch_unwind(|| panic!("removed directory")).is_err());
        }
        "failed-quota" => {
            fs::remove_dir(&crash_dir).unwrap();
            for _ in 0..20 {
                assert!(panic::catch_unwind(|| panic!("failed write attempt")).is_err());
            }
            fs::create_dir(&crash_dir).unwrap();
            assert!(panic::catch_unwind(|| panic!("failed attempts consumed quota")).is_err());
        }
        "long" => {
            let message = "猫🐈".repeat(100_000);
            assert!(panic::catch_unwind(move || panic::panic_any(message)).is_err());
        }
        "non-string" => {
            struct Payload;
            impl std::fmt::Debug for Payload {
                fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    panic!("reporter must not format arbitrary payloads");
                }
            }
            assert!(panic::catch_unwind(|| panic::panic_any(Payload)).is_err());
        }
        _ => panic!("unknown child scenario"),
    }
}

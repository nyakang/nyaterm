use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=NYATERM_COMPONENT_BUILD_ID");
    println!("cargo:rerun-if-env-changed=NYATERM_COMPONENT_TARGET");
    let component_build =
        env::var("NYATERM_COMPONENT_BUILD_ID").unwrap_or_else(|_| "development".into());
    let component_target =
        env::var("NYATERM_COMPONENT_TARGET").unwrap_or_else(|_| env::var("TARGET").unwrap());
    assert!(
        component_build
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    assert!(
        component_target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    println!("cargo:rustc-env=NYATERM_COMPONENT_BUILD_ID={component_build}");
    println!("cargo:rustc-env=NYATERM_COMPONENT_TARGET={component_target}");
    println!("cargo:rerun-if-changed=assets/completion/catalog.json");

    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("missing manifest dir"));
    let source = fs::read(manifest_dir.join("assets/completion/catalog.json"))
        .expect("failed to read completion catalog");
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&source)
        .expect("failed to compress completion catalog");
    let compressed = encoder
        .finish()
        .expect("failed to finish completion catalog compression");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("missing build output dir"));
    fs::write(out_dir.join("catalog.json.zlib"), compressed)
        .expect("failed to write compressed completion catalog");
}

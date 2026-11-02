fn main() {
    println!("cargo:rerun-if-changed=native");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    #[cfg(windows)]
    build();
    #[cfg(not(windows))]
    panic!("the system RDP type library requires a Windows build host");
}

#[cfg(windows)]
fn build() {
    use std::path::PathBuf;
    let target = std::env::var("TARGET").expect("Cargo target");
    assert!(target.ends_with("windows-msvc"), "RDP requires MSVC");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory"));
    println!("cargo:rerun-if-env-changed=VCToolsInstallDir");
    let compiler = cc::Build::new().cpp(true).get_compiler();
    assert!(
        compiler.is_like_msvc(),
        "RDP requires the MSVC C++ compiler"
    );
    // Cargo normally discovers Visual Studio without a developer shell. The
    // compiler lives under <tools>/bin/Host<host>/<target>/cl.exe.
    let tools = std::env::var_os("VCToolsInstallDir")
        .map(PathBuf::from)
        .or_else(|| compiler.path().ancestors().nth(4).map(PathBuf::from))
        .expect("locate the installed MSVC toolchain");
    let configure = || {
        let mut build = cc::Build::new();
        build
            .cpp(true)
            .include("native")
            .include(tools.join("atlmfc/include"))
            .include(&out)
            .out_dir(&out)
            .flag("/std:c++17")
            .flag("/EHsc")
            .flag("/permissive-")
            .flag("/utf-8")
            .define("UNICODE", None)
            .define("_UNICODE", None);
        build
    };
    configure()
        .file("native/import.cpp")
        .try_compile_intermediates()
        .expect("import the installed mstscax.dll type library");
    configure()
        .file("native/host.cpp")
        .compile("nyaterm_windows_rdp");
    let arch = if target.starts_with("aarch64") {
        "arm64"
    } else {
        "x64"
    };
    let lib = tools.join("atlmfc/lib").join(arch);
    assert!(
        lib.join("atls.lib").is_file(),
        "install MSVC ATL for {arch}"
    );
    println!("cargo:rustc-link-search=native={}", lib.display());
    for library in ["atls", "ole32", "oleaut32", "user32", "uuid", "comctl32"] {
        println!("cargo:rustc-link-lib={library}");
    }
}

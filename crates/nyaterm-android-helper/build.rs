fn main() {
    let target = std::env::var("TARGET").expect("TARGET");
    if target.contains("linux") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/android-libs:$ORIGIN/../android-libs");
    } else if target.contains("apple") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/android-libs");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/../android-libs");
    }
}

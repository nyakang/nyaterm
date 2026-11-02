#[cfg(windows)]
mod windows_smoke;

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    windows_smoke::run()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The native overlay smoke program requires Windows.");
}

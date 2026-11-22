use nyaterm_spice_helper::native::NativeRuntime;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: this executable has not created threads or loaded native code.
    unsafe { nyaterm_spice_helper::native::environment::initialize_process_environment()? };
    let runtime = NativeRuntime::load()?;
    let probe = runtime.probe()?;
    if std::env::args().any(|argument| argument == "--require-video")
        && probe.video_factories.len() != 10
    {
        return Err("packaged SPICE video factories are incomplete".into());
    }
    println!("{}", serde_json::to_string(&probe)?);
    Ok(())
}

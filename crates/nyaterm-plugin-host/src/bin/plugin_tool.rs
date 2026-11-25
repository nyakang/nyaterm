use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use nyaterm_plugin_host::package::{StagingDirectory, load, snapshot};
use nyaterm_plugin_host::runtime::RuntimeEngine;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [operation, input, output] if operation == "component" => {
            let module = fs::read(input)?;
            let component = wit_component::ComponentEncoder::default().module(&module)?.validate(true).encode()?;
            nyaterm_plugin_host::runtime::validate_api_marker(&component)?;
            fs::write(output, component)?;
            println!("Built component: {output}");
        }
        [operation, input] if operation == "validate" => validate(Path::new(input))?,
        [operation, input] if operation == "inspect" => inspect(Path::new(input), true)?,
        [operation, input] if operation == "catalog" => {
            let bytes = fs::read(input)?;
            let catalog = nyaterm_core::plugins::catalog::Catalog::parse(&bytes)?;
            println!("{}", serde_json::json!({"protocol_version": 2, "catalog": catalog}));
        }
        [operation, input, output] if operation == "pack" => {
            validate(Path::new(input))?;
            let temporary = temporary_root()?;
            let result = (|| {
                let stage = StagingDirectory::new(&temporary)?;
                snapshot(Path::new(input), &stage.path)?;
                let mut writer = zip::ZipWriter::new(fs::File::create(output)?);
                write_archive(&stage.path, &stage.path, &mut writer)?;
                writer.finish()?;
                Ok::<_, Box<dyn std::error::Error>>(())
            })();
            fs::remove_dir_all(&temporary)?;
            result?;
            println!("Packaged plugin: {output}");
        }
        _ => return Err("Usage: nyaterm-plugin-tool component <core.wasm> <component.wasm> | inspect <directory-or.zip> | validate <directory-or.zip> | pack <directory-or.zip> <output.zip>".into()),
    }
    Ok(())
}

fn temporary_root() -> Result<PathBuf, std::io::Error> {
    let root = std::env::temp_dir().join(format!("nyaterm-plugin-tool-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root)?;
    Ok(root)
}

fn validate(input: &Path) -> Result<(), Box<dyn std::error::Error>> {
    inspect(input, false)
}

fn inspect(input: &Path, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let root = temporary_root()?;
    let result = (|| {
        let stage = StagingDirectory::new(&root)?;
        snapshot(input, &stage.path)?;
        let package = load(&stage.path, env!("CARGO_PKG_VERSION"))?;
        if let Some(component) = &package.component {
            RuntimeEngine::new()?.validate_component(component)?;
        }
        let manifest = package.manifest;
        if json {
            println!(
                "{}",
                serde_json::json!({"protocol_version": 2, "manifest": manifest})
            );
            return Ok(());
        }
        println!(
            "Validated {} {} (API {}, {} actions)",
            manifest.id,
            manifest.version,
            manifest.api_version,
            manifest.actions.len()
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    fs::remove_dir_all(&root)?;
    result
}

fn write_archive(
    root: &Path,
    path: &Path,
    writer: &mut zip::ZipWriter<fs::File>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            write_archive(root, &path, writer)?;
        } else {
            let name = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("Non-UTF8 resource path")?
                .replace('\\', "/");
            writer.start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )?;
            writer.write_all(&fs::read(path)?)?;
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

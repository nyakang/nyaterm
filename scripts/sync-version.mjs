import { readFileSync, writeFileSync } from 'fs';
import { execSync } from 'child_process';

const pkg = JSON.parse(readFileSync('package.json', 'utf-8'));
const version = pkg.version;

// Change the version in tauri.conf.json
const tauriConf = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf-8'));
tauriConf.version = version;
writeFileSync('src-tauri/tauri.conf.json', JSON.stringify(tauriConf, null, 2) + '\n');

// Change the version in Cargo.toml
let cargo = readFileSync('src-tauri/Cargo.toml', 'utf-8');
cargo = cargo.replace(
    /(\[package\]\s*\nname\s*=\s*"[^"]*"\s*\n)version\s*=\s*"[^"]*"/,
    `$1version = "${version}"`
);
writeFileSync('src-tauri/Cargo.toml', cargo);

// Keep the separately-built MCP sidecar package aligned with the app release.
const sidecarManifestPath = 'src-tauri/crates/nyaterm-mcp/Cargo.toml';
let sidecarCargo = readFileSync(sidecarManifestPath, 'utf-8');
sidecarCargo = sidecarCargo.replace(
    /(\[package\]\s*\nname\s*=\s*"nyaterm-mcp"\s*\n)version\s*=\s*"[^"]*"/,
    `$1version = "${version}"`
);
writeFileSync(sidecarManifestPath, sidecarCargo);

// Change the version in Cargo.lock
function updateNyaTermVersion(version) {
  const filePath = 'src-tauri/Cargo.lock';
  const content = readFileSync(filePath, 'utf-8');

  const pattern =
    /(\[\[package\]\]\r?\nname = "nyaterm"\r?\nversion = ")([^"]*)(")/;

  if (!pattern.test(content)) {
    throw new Error(
      'Could not find the version field for [[package]] name = "nyaterm" in src-tauri/Cargo.lock'
    );
  }

  const updated = content.replace(pattern, `$1${version}$3`);

  writeFileSync(filePath, updated, 'utf-8');
}
updateNyaTermVersion(version);

function updateSidecarLockVersion(version) {
  const filePath = 'src-tauri/crates/nyaterm-mcp/Cargo.lock';
  const content = readFileSync(filePath, 'utf-8');
  const pattern =
    /(\[\[package\]\]\r?\nname = "nyaterm-mcp"\r?\nversion = ")([^"]*)(")/;
  if (!pattern.test(content)) {
    throw new Error('Could not find nyaterm-mcp in the sidecar Cargo.lock');
  }
  writeFileSync(filePath, content.replace(pattern, `$1${version}$3`), 'utf-8');
}
updateSidecarLockVersion(version);

console.log(`✅ Version synced to ${version}`);

// If the --commit parameter is passed, automatically commit the version change
if (process.argv.includes('--commit')) {
    const files = [
      'package.json',
      'src-tauri/tauri.conf.json',
      'src-tauri/Cargo.toml',
      'src-tauri/Cargo.lock',
      sidecarManifestPath,
      'src-tauri/crates/nyaterm-mcp/Cargo.lock'
    ];
    execSync(`git add ${files.join(' ')}`, { stdio: 'inherit' });
    execSync(`git commit -m "chore: bump version to v${version}"`, { stdio: 'inherit' });
    console.log(`✅ Committed: chore: bump version to v${version}`);
}

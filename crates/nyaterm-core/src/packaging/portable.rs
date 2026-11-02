//! Portable package contract shared with the Python packager and verifier.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::updater::UpdateArchitecture;

#[derive(Deserialize)]
pub struct PortableLayout {
    pub root: String,
    pub marker: String,
    pub data_placeholder: String,
    program_files: Vec<String>,
    helper_files: Vec<String>,
    support_files: Vec<String>,
    conpty_files: BTreeMap<String, Vec<String>>,
}

impl PortableLayout {
    /// Program payload only: user data is never installed from an update archive.
    pub fn files(&self, architecture: UpdateArchitecture) -> Vec<&str> {
        let target = match architecture {
            UpdateArchitecture::X86_64 => "x86_64-pc-windows-msvc",
            UpdateArchitecture::Aarch64 => "aarch64-pc-windows-msvc",
        };
        self.program_files
            .iter()
            .chain(&self.helper_files)
            .chain(&self.support_files)
            .chain(&self.conpty_files[target])
            .map(String::as_str)
            .chain(std::iter::once(self.marker.as_str()))
            .collect()
    }

    pub fn data_directory(&self) -> &str {
        self.data_placeholder
            .split('/')
            .next()
            .expect("data directory")
    }
}

pub fn windows_portable_layout() -> &'static PortableLayout {
    static LAYOUT: OnceLock<PortableLayout> = OnceLock::new();
    LAYOUT.get_or_init(|| {
        serde_json::from_str(include_str!("windows-portable.json"))
            .expect("bundled portable package contract must be valid")
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::windows_portable_layout;
    use crate::updater::UpdateArchitecture;

    #[test]
    fn portable_contract_has_safe_unique_files_separate_from_user_data() {
        let layout = windows_portable_layout();
        for architecture in [UpdateArchitecture::X86_64, UpdateArchitecture::Aarch64] {
            let mut found = BTreeSet::new();
            for name in layout.files(architecture) {
                assert!(name.split('/').all(|part| {
                    !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':'])
                }));
                assert!(!name.starts_with(&format!("{}/", layout.data_directory())));
                assert!(found.insert(name.to_ascii_lowercase()), "duplicate {name}");
            }
            assert!(found.contains("nyaterm-activex-helper.exe"));
            assert!(found.contains("conpty/license.txt"));
        }
    }
}

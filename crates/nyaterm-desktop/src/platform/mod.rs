#[cfg(target_os = "linux")]
pub(crate) mod linux;

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) mod windows_acl;

#[derive(Debug, Clone, Copy)]
pub(crate) struct WindowMaterialCapabilities {
    pub transparent: bool,
    pub blurred: bool,
}

/// Local client session information, never a property of the remote server.
pub(crate) fn is_remote_session() -> Option<bool> {
    #[cfg(windows)]
    unsafe {
        Some(
            windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
                windows_sys::Win32::UI::WindowsAndMessaging::SM_REMOTESESSION,
            ) != 0,
        )
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RendererKind {
    Hardware,
    Software,
    #[default]
    Unknown,
}

impl RendererKind {
    pub(crate) fn from_specs(specs: Option<&gpui::GpuSpecs>) -> Self {
        match specs {
            Some(specs) if specs.is_software_emulated => Self::Software,
            Some(specs) if !specs.device_name.trim().is_empty() => Self::Hardware,
            _ => Self::Unknown,
        }
    }
}

/// Windows reports WARP through the Basic Render Driver without always setting
/// the DXGI software-adapter flag, so the adapter name is part of the check.
pub(crate) fn is_software_renderer(specs: Option<&gpui::GpuSpecs>) -> bool {
    specs.is_some_and(|specs| {
        specs.is_software_emulated
            || specs
                .device_name
                .trim()
                .eq_ignore_ascii_case("Microsoft Basic Render Driver")
    })
}

/// Probe once outside rendering. GPUI exposes no result for applying a material,
/// so these describe known support, not confirmation of the visible effect.
pub(crate) fn window_material_capabilities() -> WindowMaterialCapabilities {
    static CAPABILITIES: std::sync::OnceLock<WindowMaterialCapabilities> =
        std::sync::OnceLock::new();
    *CAPABILITIES.get_or_init(|| {
        #[cfg(windows)]
        {
            windows::window_material_capabilities()
        }
        #[cfg(target_os = "macos")]
        {
            WindowMaterialCapabilities {
                transparent: true,
                blurred: true,
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            WindowMaterialCapabilities {
                transparent: false,
                blurred: false,
            }
        }
    })
}

/// Allow native menus to follow the system theme before creating UI windows.
pub fn init_native_menu_theme() {
    #[cfg(windows)]
    windows::enable_native_menu_dark_mode();
}

#[cfg(test)]
mod tests {
    use super::{RendererKind, is_software_renderer};

    #[test]
    fn renderer_kind_uses_device_flags_and_preserves_unknown() {
        assert_eq!(RendererKind::from_specs(None), RendererKind::Unknown);
        let mut specs = gpui::GpuSpecs::default();
        assert_eq!(
            RendererKind::from_specs(Some(&specs)),
            RendererKind::Unknown
        );
        specs.device_name = "Microsoft Basic Render Driver".into();
        assert_eq!(
            RendererKind::from_specs(Some(&specs)),
            RendererKind::Hardware
        );
        specs.is_software_emulated = true;
        specs.device_name = "arbitrary device name".into();
        assert_eq!(
            RendererKind::from_specs(Some(&specs)),
            RendererKind::Software
        );
    }

    #[test]
    fn basic_render_driver_counts_as_software_even_without_the_flag() {
        let mut specs = gpui::GpuSpecs {
            device_name: "Microsoft Basic Render Driver".into(),
            ..Default::default()
        };
        assert!(is_software_renderer(Some(&specs)));

        specs.device_name = "AMD Radeon RX 580 Series".into();
        assert!(!is_software_renderer(Some(&specs)));
        specs.is_software_emulated = true;
        assert!(is_software_renderer(Some(&specs)));
    }
}

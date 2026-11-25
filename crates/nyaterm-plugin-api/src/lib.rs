//! Plugin ABI constants and optional guest SDK, derived from the host's WIT.
pub mod protocol;

#[cfg(feature = "guest")]
pub mod host {
    use crate::protocol::{HostRequest, TaskEvent};

    pub fn submit(request: &HostRequest) -> Result<u64, String> {
        let bytes = serde_json::to_vec(request).map_err(|_| "Cannot encode host request")?;
        crate::bindings::nyaterm::plugin::host::submit(&bytes)
    }

    pub fn poll(task: u64) -> Result<TaskEvent, String> {
        let bytes = crate::bindings::nyaterm::plugin::host::poll(task)?;
        serde_json::from_slice(&bytes).map_err(|_| "Invalid host response".into())
    }

    pub fn cancel(task: u64) -> Result<(), String> {
        crate::bindings::nyaterm::plugin::host::cancel(task)
    }
}
pub mod abi {
    include!(concat!(env!("OUT_DIR"), "/abi_version.rs"));
}

#[cfg(feature = "guest")]
pub mod bindings {
    wit_bindgen::generate!({ path: "wit", world: "plugin", pub_export_macro: true });
}

#[cfg(feature = "guest")]
pub use bindings::nyaterm::plugin::types::{
    ActionInput, ActionResult, Argument, CommandDraft, ErrorKind, Identity, PluginError, Value,
    Version,
};

#[cfg(feature = "guest")]
pub const API_VERSION: Version = Version {
    major: abi::API_VERSION_PARTS.0,
    minor: abi::API_VERSION_PARTS.1,
    patch: abi::API_VERSION_PARTS.2,
};

#[cfg(feature = "guest")]
pub trait Plugin: Default {
    fn initialize(&mut self, _identity: Identity) -> Result<(), PluginError> {
        Ok(())
    }
    fn invoke(&mut self, input: ActionInput) -> Result<ActionResult, PluginError>;
    fn task_ready(&mut self, _task: u64) {}
    fn shutdown(&mut self) {}
}

/// Register one stateful guest. Wasmtime serializes all calls to this instance.
#[macro_export]
#[cfg(feature = "guest")]
macro_rules! register_plugin {
    ($plugin:ty) => {
        #[cfg(target_arch = "wasm32")]
        #[used]
        #[unsafe(link_section = "nyaterm:plugin-api")]
        static NYATERM_PLUGIN_API_VERSION: [u8; $crate::abi::API_VERSION_BYTES.len()] =
            $crate::abi::API_VERSION_BYTES;

        struct NyaTermGuest;
        std::thread_local! {
            static NYATERM_PLUGIN: std::cell::RefCell<$plugin> = std::cell::RefCell::new(<$plugin as Default>::default());
        }
        impl $crate::bindings::Guest for NyaTermGuest {
            fn api_version() -> $crate::Version { $crate::API_VERSION }
            fn initialize(identity: $crate::Identity) -> Result<(), $crate::PluginError> {
                NYATERM_PLUGIN.with(|p| $crate::Plugin::initialize(&mut *p.borrow_mut(), identity))
            }
            fn invoke(input: $crate::ActionInput) -> Result<$crate::ActionResult, $crate::PluginError> {
                NYATERM_PLUGIN.with(|p| $crate::Plugin::invoke(&mut *p.borrow_mut(), input))
            }
            fn shutdown() { NYATERM_PLUGIN.with(|p| $crate::Plugin::shutdown(&mut *p.borrow_mut())); }
            fn task_ready(task: u64) { NYATERM_PLUGIN.with(|p| $crate::Plugin::task_ready(&mut *p.borrow_mut(), task)); }
        }
        $crate::bindings::export!(NyaTermGuest with_types_in $crate::bindings);
    };
}

#[cfg(all(test, feature = "guest"))]
mod tests {
    use crate::{API_VERSION, abi};

    #[test]
    fn sdk_version_and_marker_match_the_wit_package() {
        let declaration = format!("package nyaterm:plugin@{};", abi::API_VERSION_TEXT);
        assert!(
            include_str!("../wit/plugin.wit")
                .lines()
                .any(|line| line.trim() == declaration)
        );
        assert_eq!(
            abi::API_VERSION_BYTES.as_slice(),
            abi::API_VERSION_TEXT.as_bytes()
        );
        assert_eq!(
            (API_VERSION.major, API_VERSION.minor, API_VERSION.patch),
            abi::API_VERSION_PARTS
        );
    }
}

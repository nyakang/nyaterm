use crate::models::session::AndroidSessionConfig;
use gpui::{Bounds, DynamicTexture, Entity, FocusHandle, Pixels};
use nyaterm_android::devices::SharedDeviceService;
use nyaterm_android::ipc::Frame;
use nyaterm_android::screen::{CleanupTicket, ScreenHandle};
use nyaterm_core::models::android::AndroidViewKind;
use nyaterm_files::adb::AdbFileService;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub(in crate::features) mod labels;
mod runtime;
mod surface;
#[cfg(test)]
mod tests;
mod view;

// Attempts survive window transfers; a window-local counter can accept stale results.
fn next_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub(in crate::features) struct DeviceLease {
    pub config: AndroidSessionConfig,
    pub files: Arc<AdbFileService>,
    pub observed: bool,
    pub owners: Arc<()>,
}
pub(in crate::features) struct AndroidView {
    pub device_key: String,
    pub kind: AndroidViewKind,
}
pub(super) struct ScreenState {
    focus: FocusHandle,
    focus_out: Option<gpui::Subscription>,
    handle: Option<ScreenHandle>,
    attempt: u64,
    physical_keys: HashSet<u32>,
    frame: Option<Frame>,
    texture: Option<DynamicTexture>,
    texture_size: Option<(u32, u32)>,
    bounds: Option<Bounds<Pixels>>,
    bounds_generation: u64,
    generation: u64,
    dimensions: (u32, u32),
    drag: bool,
    error: Option<labels::AndroidError>,
    start_control: nyaterm_android::operation::AdbOperationControl,
    visible: bool,
    error_reported: bool,
    last_input_notice: Option<Instant>,
    viewport: nyaterm_core::models::android::ScreenViewport,
    viewport_bounds: Option<Bounds<Pixels>>,
    layout_epoch: u64,
    pan_drag: Option<gpui::Point<Pixels>>,
    paste_shortcut: bool,
}
impl ScreenState {
    fn new(
        focus: FocusHandle,
        attempt: u64,
        start_control: nyaterm_android::operation::AdbOperationControl,
    ) -> Self {
        Self {
            focus,
            focus_out: None,
            handle: None,
            attempt,
            physical_keys: HashSet::new(),
            frame: None,
            texture: None,
            texture_size: None,
            bounds: None,
            bounds_generation: 0,
            generation: 0,
            dimensions: (0, 0),
            drag: false,
            error: None,
            start_control,
            visible: true,
            error_reported: false,
            last_input_notice: None,
            viewport: Default::default(),
            viewport_bounds: None,
            layout_epoch: 0,
            pan_drag: None,
            paste_shortcut: false,
        }
    }
}
pub(in crate::features) struct AndroidFeatureState {
    pub(super) devices: HashMap<String, DeviceLease>,
    pub(super) views: HashMap<String, AndroidView>,
    screens: HashMap<String, ScreenState>,
    surfaces: HashMap<String, Entity<surface::AndroidSurface>>,
    cleanup: Vec<CleanupTicket>,
    pub(super) retired_textures: Vec<DynamicTexture>,
    pub(in crate::features) pending: HashMap<String, String>,
    pub(super) tracker: SharedDeviceService,
    pub(super) clock: Option<gpui::Task<()>>,
    pub(super) tracker_revision: u64,
}
impl AndroidFeatureState {
    pub(in crate::features) fn new(tracker: SharedDeviceService) -> Self {
        Self {
            devices: HashMap::new(),
            views: HashMap::new(),
            screens: HashMap::new(),
            surfaces: HashMap::new(),
            cleanup: Vec::new(),
            retired_textures: Vec::new(),
            pending: HashMap::new(),
            tracker,
            clock: None,
            tracker_revision: 0,
        }
    }
    pub(in crate::features) fn file_service(
        &self,
        key: &str,
    ) -> anyhow::Result<Arc<AdbFileService>> {
        self.devices
            .get(key)
            .map(|device| device.files.clone())
            .ok_or_else(|| anyhow::Error::new(nyaterm_android::error::AdbError::Offline))
    }
    pub(in crate::features) fn device_name(&self, key: &str) -> String {
        self.devices
            .get(key)
            .map(|d| d.config.name.clone())
            .unwrap_or_else(|| rust_i18n::t!("android.disconnectedTitle").into_owned())
    }

    pub(in crate::features) fn key_for_file_service(
        &self,
        service: &std::sync::Weak<AdbFileService>,
    ) -> Option<String> {
        self.devices.iter().find_map(|(key, device)| {
            std::sync::Weak::ptr_eq(service, &Arc::downgrade(&device.files)).then(|| key.clone())
        })
    }
    pub(in crate::features) fn kind(&self, id: &str) -> Option<AndroidViewKind> {
        self.views.get(id).map(|v| v.kind)
    }
    pub(super) fn collect_unused_devices(&mut self, pending_shells: &HashSet<String>) {
        self.cleanup.retain(|ticket| !ticket.is_complete());
        self.devices.retain(|key, lease| {
            self.views.values().any(|view| &view.device_key == key)
                || self.pending.contains_key(key)
                || pending_shells.contains(key)
                || Arc::strong_count(&lease.files) > Arc::strong_count(&lease.owners)
        });
    }
    pub(in crate::features) fn close_view(&mut self, id: &str) {
        self.views.remove(id);
        self.surfaces.remove(id);
        if let Some(mut screen) = self.screens.remove(id) {
            screen.start_control.cancel();
            if let Some(texture) = screen.texture.take() {
                self.retired_textures.push(texture);
            }
            if let Some(handle) = screen.handle {
                handle.close();
                self.cleanup.push(handle.cleanup_ticket.clone());
            }
        }
    }
    pub(in crate::features) fn stop_screen(&mut self, id: &str) {
        if let Some(screen) = self.screens.get_mut(id) {
            screen.start_control.cancel();
            screen.attempt = next_generation();
            screen.drag = false;
            screen.pan_drag = None;
            screen.paste_shortcut = false;
            screen.physical_keys.clear();
            if let Some(handle) = screen.handle.take() {
                handle.close();
                self.cleanup.push(handle.cleanup_ticket.clone());
            }
        }
    }
    pub(in crate::features) fn release_inputs(&mut self, id: &str) {
        if let Some(screen) = self.screens.get_mut(id) {
            screen.drag = false;
            screen.pan_drag = None;
            screen.paste_shortcut = false;
            screen.physical_keys.clear();
            if let Some(handle) = &screen.handle {
                let _ = handle.send(nyaterm_android::ipc::Control::ReleaseInputs);
            }
        }
    }
    pub(in crate::features) fn release_all_inputs(&mut self) {
        let ids: Vec<_> = self.screens.keys().cloned().collect();
        for id in ids {
            self.release_inputs(&id);
        }
    }
    pub(in crate::features) fn shutdown(&mut self) {
        self.clock.take();
        self.pending.clear();
        for device in self.devices.values() {
            if Arc::strong_count(&device.owners) == 1 {
                device.files.cancel();
            }
        }
        for screen in self.screens.values() {
            screen.start_control.cancel();
            if let Some(handle) = &screen.handle {
                handle.close();
            }
        }
    }
}

struct AndroidTransferredView {
    id: String,
    view: AndroidView,
    screen: Option<ScreenState>,
    workspace: Option<Entity<crate::features::file_manager::FileManagerWorkspace>>,
}
pub(crate) struct AndroidTabTransfer {
    views: Vec<AndroidTransferredView>,
    devices: Vec<(String, DeviceLease)>,
}
impl AndroidFeatureState {
    pub(crate) fn detach_tab(
        &mut self,
        ids: &[String],
        files: &mut crate::features::file_manager::FileManagerFeatureState,
    ) -> AndroidTabTransfer {
        let mut views = Vec::new();
        let mut keys = HashSet::new();
        for id in ids {
            self.release_inputs(id);
            self.surfaces.remove(id);
            if let Some(view) = self.views.remove(id) {
                keys.insert(view.device_key.clone());
                let mut screen = self.screens.remove(id);
                if let Some(screen) = &mut screen {
                    if screen.handle.is_none() {
                        screen.start_control.cancel();
                        screen.error = Some(labels::AndroidError::Disconnected);
                    }
                    if let Some(texture) = screen.texture.take() {
                        self.retired_textures.push(texture);
                    }
                    screen.texture_size = None;
                    screen.bounds = None;
                    screen.viewport_bounds = None;
                    screen.focus_out = None;
                }
                views.push(AndroidTransferredView {
                    id: id.clone(),
                    view,
                    screen,
                    workspace: files.take_embedded(id),
                });
            }
        }
        let devices: Vec<(String, DeviceLease)> = keys
            .into_iter()
            .filter_map(|key| {
                self.devices.get(&key).map(|lease| {
                    (
                        key,
                        DeviceLease {
                            config: lease.config.clone(),
                            files: lease.files.clone(),
                            observed: lease.observed,
                            owners: lease.owners.clone(),
                        },
                    )
                })
            })
            .collect();
        for (key, _) in &devices {
            if !self.views.values().any(|view| &view.device_key == key) {
                self.devices.remove(key);
            }
        }
        AndroidTabTransfer { views, devices }
    }
    pub(crate) fn attach_tab(
        &mut self,
        bundle: AndroidTabTransfer,
        files: &mut crate::features::file_manager::FileManagerFeatureState,
        app: Entity<crate::features::NyaTermApp>,
        cx: &mut gpui::App,
    ) {
        self.tracker_revision = 0;
        for (key, lease) in bundle.devices {
            match self.devices.entry(key) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(lease);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    // A dead binding cannot protect live channels. Prefer the valid
                    // incoming lease without reviving views tied to the old one.
                    if !entry.get().files.runtime.is_valid() && lease.files.runtime.is_valid() {
                        entry.insert(lease);
                    }
                }
            }
        }
        for AndroidTransferredView {
            id,
            view,
            screen,
            workspace,
        } in bundle.views
        {
            self.views.insert(id.clone(), view);
            if let Some(mut screen) = screen {
                screen.focus = cx.focus_handle();
                screen.focus_out = None;
                if let Some(handle) = &screen.handle {
                    let _ = handle.send(nyaterm_android::ipc::Control::RefreshPresentation);
                }
                self.screens.insert(id.clone(), screen);
            }
            if let Some(workspace) = workspace {
                workspace.update(cx, |workspace, cx| workspace.rebind_app(app.clone(), cx));
                files.insert_android(id, workspace);
            }
        }
    }

    pub(crate) fn can_attach_tab(&self, bundle: &AndroidTabTransfer) -> bool {
        bundle.devices.iter().all(|(key, lease)| {
            self.devices.get(key).is_none_or(|existing| {
                Arc::ptr_eq(&existing.files, &lease.files)
                    || !existing.files.runtime.is_valid()
                    || !lease.files.runtime.is_valid()
            })
        })
    }
}

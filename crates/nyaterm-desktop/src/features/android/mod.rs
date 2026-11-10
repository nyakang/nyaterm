use crate::models::session::AndroidSessionConfig;
use gpui::{Bounds, DynamicTexture, Entity, FocusHandle, Pixels};
use nyaterm_android::devices::SharedDeviceService;
use nyaterm_android::ipc::Frame;
use nyaterm_android::screen::{CleanupTicket, ScreenHandle};
use nyaterm_core::models::android::AndroidViewKind;
use nyaterm_files::adb::AdbFileService;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

pub(in crate::features) mod labels;
mod runtime;
mod surface;
#[cfg(test)]
mod tests;
mod view;

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
    last_render: Instant,
    zoom: f32,
}
pub(in crate::features) struct AndroidFeatureState {
    pub(super) devices: HashMap<String, DeviceLease>,
    pub(super) views: HashMap<String, AndroidView>,
    screens: HashMap<String, ScreenState>,
    surfaces: HashMap<String, Entity<surface::AndroidSurface>>,
    cleanup: Vec<CleanupTicket>,
    pub(super) retired_textures: Vec<DynamicTexture>,
    pub(super) focus_out: Option<gpui::Subscription>,
    pub(in crate::features) pending: HashMap<String, u64>,
    generation: u64,
    pub(super) tracker: SharedDeviceService,
    pub(super) clock: Option<gpui::Task<()>>,
    pub(super) tracker_revision: u64,
    pub(super) focus: FocusHandle,
}
impl AndroidFeatureState {
    pub(in crate::features) fn new(tracker: SharedDeviceService, focus: FocusHandle) -> Self {
        Self {
            devices: HashMap::new(),
            views: HashMap::new(),
            screens: HashMap::new(),
            surfaces: HashMap::new(),
            cleanup: Vec::new(),
            retired_textures: Vec::new(),
            focus_out: None,
            pending: HashMap::new(),
            generation: 0,
            tracker,
            clock: None,
            tracker_revision: 0,
            focus,
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
    pub(in crate::features) fn kind(&self, id: &str) -> Option<AndroidViewKind> {
        self.views.get(id).map(|v| v.kind)
    }
    pub(in crate::features) fn close_view(&mut self, id: &str) {
        self.views.remove(id);
        self.surfaces.remove(id);
        if let Some(mut screen) = self.screens.remove(id) {
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
            screen.attempt = screen.attempt.wrapping_add(1);
            screen.drag = false;
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
            screen.physical_keys.clear();
            if let Some(handle) = &screen.handle {
                let _ = handle.send(nyaterm_android::ipc::Control::ReleaseInputs);
            }
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
                    if let Some(texture) = screen.texture.take() {
                        self.retired_textures.push(texture);
                    }
                    screen.texture_size = None;
                    screen.bounds = None;
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
        self.devices.extend(bundle.devices);
        for AndroidTransferredView {
            id,
            view,
            screen,
            workspace,
        } in bundle.views
        {
            self.views.insert(id.clone(), view);
            if let Some(screen) = screen {
                self.screens.insert(id.clone(), screen);
            }
            if let Some(workspace) = workspace {
                workspace.update(cx, |workspace, cx| workspace.rebind_app(app.clone(), cx));
                files.insert_android(id, workspace);
            }
        }
    }
}

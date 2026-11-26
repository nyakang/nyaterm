use crate::models::session::AndroidSessionConfig;
use gpui::{Bounds, DynamicTexture, Entity, FocusHandle, Pixels};
use nyaterm_android::devices::{DeviceTrackerLease, SharedDeviceService};
use nyaterm_android::ipc::Frame;
use nyaterm_android::screen::{CleanupTicket, ScreenHandle};
use nyaterm_core::models::android::AndroidViewKind;
use nyaterm_core::models::android::gesture::ScreenGesture;
use nyaterm_files::adb::AdbFileService;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

mod input;
pub(in crate::features) mod labels;
mod runtime;
mod settings;
mod surface;
#[cfg(test)]
mod tests;
mod toolbar;
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
#[derive(Clone, Copy)]
pub(in crate::features) enum ScreenTexture {
    Bgra(DynamicTexture),
    Video(gpui::VideoTexture, nyaterm_android::ipc::FrameColor),
}
impl ScreenTexture {
    pub(in crate::features) fn remove(self, window: &mut gpui::Window) {
        match self {
            Self::Bgra(texture) => window.remove_dynamic_texture(texture),
            Self::Video(texture, _) => window.remove_video_texture(texture),
        }
    }
    fn paint(self, bounds: Bounds<Pixels>, window: &mut gpui::Window) -> anyhow::Result<()> {
        match self {
            Self::Bgra(texture) => window.paint_dynamic_texture(bounds, texture),
            Self::Video(texture, color) => {
                let matrix = match color.matrix {
                    nyaterm_android::ipc::ColorMatrix::Bt601 => gpui::VideoColorMatrix::Bt601,
                    nyaterm_android::ipc::ColorMatrix::Bt709 => gpui::VideoColorMatrix::Bt709,
                    nyaterm_android::ipc::ColorMatrix::Bt2020 => gpui::VideoColorMatrix::Bt2020,
                };
                window.paint_video_texture(bounds, texture, matrix, color.full_range)
            }
        }
    }
}
pub(super) struct ScreenState {
    focus: FocusHandle,
    focus_out: Option<gpui::Subscription>,
    handle: Option<ScreenHandle>,
    attempt: u64,
    physical_keys: HashSet<u32>,
    frame: Option<Frame>,
    texture: Option<ScreenTexture>,
    texture_size: Option<(u32, u32)>,
    requested_format: Option<nyaterm_android::ipc::PixelFormat>,
    bounds: Option<Bounds<Pixels>>,
    bounds_generation: u64,
    generation: u64,
    dimensions: (u32, u32),
    gesture: Option<ScreenGesture>,
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
    settings: nyaterm_core::models::android::AndroidScreenSettings,
    settings_draft: Option<settings::ScreenSettingsDraft>,
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
            requested_format: None,
            bounds: None,
            bounds_generation: 0,
            generation: 0,
            dimensions: (0, 0),
            gesture: None,
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
            settings: Default::default(),
            settings_draft: None,
        }
    }
}
pub(in crate::features) struct AndroidFeatureState {
    pub(super) devices: HashMap<String, DeviceLease>,
    pub(super) views: HashMap<String, AndroidView>,
    screens: HashMap<String, ScreenState>,
    surfaces: HashMap<String, Entity<surface::AndroidSurface>>,
    cleanup: Vec<CleanupTicket>,
    pub(super) retired_textures: Vec<ScreenTexture>,
    pub(in crate::features) pending: HashMap<String, String>,
    pub(super) tracker: SharedDeviceService,
    pub(super) clock: Option<gpui::Task<()>>,
    pub(super) tracker_revision: (u64, u64),
    /// Editor/session demand for this window. File and screen jobs retain
    /// their own leases until their actual work and cleanup finish.
    discovery_lease: Option<DeviceTrackerLease>,
    /// An ensure job is on the blocking pool; do not queue a second one.
    pub(super) tracker_start_pending:
        Option<(u64, nyaterm_android::operation::AdbOperationControl)>,
    /// Earliest retry after a failed start, so a broken adb cannot turn the
    /// 200 ms tick into a retry storm.
    pub(super) tracker_retry_after: Option<std::time::Instant>,
    /// Acquisition/startup failure shown by the picker until retry or reopen.
    pub(super) tracker_error: Option<String>,
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
            tracker_revision: (0, 0),
            discovery_lease: None,
            tracker_start_pending: None,
            tracker_retry_after: None,
            tracker_error: None,
        }
    }

    /// Only live snapshots establish device presence. Old lists are not reused
    /// across discovery lifetimes, including while a new tracker is starting.
    pub(in crate::features) fn device_snapshot(&self) -> nyaterm_android::devices::DeviceSnapshot {
        let mut snapshot = self.tracker.snapshot();
        if snapshot.tracker_generation == 0 && snapshot.error.is_none() {
            snapshot.error.clone_from(&self.tracker_error);
        }
        snapshot
    }

    /// Held discovery that has published nothing yet.
    pub(in crate::features) fn devices_are_starting(&self) -> bool {
        let snapshot = self.tracker.snapshot();
        self.holds_discovery()
            && (snapshot.tracker_generation != 0 || self.tracker_error.is_none())
            && snapshot.revision == 0
    }

    /// Matches the lease to the consumers that exist; safe to call anywhere.
    pub(in crate::features) fn sync_discovery(&mut self, needed: bool) {
        match (needed, self.discovery_lease.is_some()) {
            (true, false) => {
                self.discovery_lease = Some(self.tracker.lease());
                self.tracker_retry_after = None;
                self.tracker_error = None;
                self.tracker_revision = (0, 0);
            }
            (false, true) => {
                if let Some((_, control)) = self.tracker_start_pending.take() {
                    control.cancel();
                }
                self.discovery_lease = None;
            }
            _ => {}
        }
    }

    /// Whether this window currently consumes device discovery.
    pub(in crate::features) fn holds_discovery(&self) -> bool {
        self.discovery_lease.is_some()
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
    pub(super) fn collect_unused_devices(&mut self, consumers: &HashSet<String>) {
        self.cleanup.retain(|ticket| !ticket.is_complete());
        self.devices.retain(|key, lease| {
            consumers.contains(key)
                || self.pending.contains_key(key)
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
            screen.gesture = None;
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
            screen.gesture = None;
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
        self.sync_discovery(false);
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
    discovery: Option<DeviceTrackerLease>,
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
                    screen.requested_format = None;
                    screen.bounds = None;
                    screen.viewport_bounds = None;
                    screen.focus_out = None;
                    screen.settings_draft = None;
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
        let discovery = (!views.is_empty() || !devices.is_empty()).then(|| self.tracker.lease());
        AndroidTabTransfer {
            views,
            devices,
            discovery,
        }
    }
    pub(crate) fn attach_tab(
        &mut self,
        bundle: AndroidTabTransfer,
        files: &mut crate::features::file_manager::FileManagerFeatureState,
        app: Entity<crate::features::NyaTermApp>,
        cx: &mut gpui::App,
    ) {
        let _transfer_discovery = bundle.discovery;
        self.tracker_revision = (0, 0);
        // Hold discovery across a window move: the source releases as soon as
        // its tab is gone, which would otherwise stop adb under the session
        // that just arrived here.
        if !bundle.devices.is_empty() || !bundle.views.is_empty() {
            self.sync_discovery(true);
        }
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

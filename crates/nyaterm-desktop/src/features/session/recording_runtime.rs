use rust_i18n::t;

use std::collections::HashSet;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use gpui::Context;
use nyaterm_core::{
    AppSettingsSummary, ExistingFileBehavior as CoreExistingFileBehavior, Group,
    RecordingMode as CoreRecordingMode, RecordingRotationPolicy as CoreRecordingRotationPolicy,
};
use nyaterm_transport::{
    ExistingFileBehavior as TransportExistingFileBehavior, RecordingContext, RecordingMode,
    RecordingProfile, RecordingRotationPolicy as TransportRecordingRotationPolicy,
};
use time::OffsetDateTime;
use time::macros::format_description;

use crate::features::commands::encode_recorded_macro;
use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job};
use crate::models::{QuickCommandEditorState, SessionLaunchConfig};

impl NyaTermApp {
    pub(in crate::features) fn toggle_macro_recording_for_session(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.recording.is_macro_recording(session_id) {
            if self.commands.quick_editor().is_some() {
                self.shell
                    .set_status(t!("recording.macroEditorBusy").to_string());
                cx.notify();
                return;
            }
            let Some(chunks) = self.recording.finish_macro(session_id) else {
                return;
            };
            if chunks.is_empty() {
                self.shell
                    .set_status(t!("recording.macroEmpty").to_string());
            } else {
                let script = encode_recorded_macro(&chunks);
                self.commands
                    .open_quick_editor(QuickCommandEditorState::from_recorded_macro(
                        t!("recording.macroDefaultLabel").to_string(),
                        script,
                    ));
                self.forget_text_inputs("quick-command.editor.");
                self.open_quick_command_window(cx);
                self.shell
                    .set_status(t!("recording.macroReview").to_string());
            }
        } else if self.session.session_info(session_id).is_none()
            || self.session.is_disconnected(session_id)
            || self.remote_desktop.is_session(session_id)
            || self.security.screen_locked()
        {
            self.shell
                .set_status(t!("recording.sessionUnavailable").to_string());
        } else {
            self.recording.start_macro(session_id);
            self.shell
                .set_status(t!("recording.macroStarted").to_string());
        }
        cx.notify();
    }

    pub(in crate::features) fn toggle_active_session_recording(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.session.active_id_owned() else {
            self.shell
                .set_status(t!("recording.sessionRequired").to_string());
            cx.notify();
            return;
        };
        self.toggle_recording_for_session(&session_id, cx);
    }

    pub(in crate::features) fn toggle_recording_for_session(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.recording.busy_action(session_id).is_some() {
            return;
        }
        if self.recording.is_recording(session_id) {
            self.stop_recording_for_session(session_id, cx);
        } else {
            self.start_recording_with_profile(session_id, None, cx);
        }
    }

    pub(in crate::features) fn start_recording_for_session(
        &mut self,
        session_id: &str,
        mode: RecordingMode,
        cx: &mut Context<Self>,
    ) {
        self.start_recording_with_profile(session_id, Some(mode), cx);
    }

    fn start_recording_with_profile(
        &mut self,
        session_id: &str,
        requested_mode: Option<RecordingMode>,
        cx: &mut Context<Self>,
    ) {
        if self.remote_desktop.is_session(session_id) {
            self.shell
                .set_status(t!("recording.unsupported").to_string());
            cx.notify();
            return;
        }
        if !self.recording.begin_action(session_id, "record") {
            self.shell.set_status(t!("recording.busy").to_string());
            cx.notify();
            return;
        }
        let Some((context, mut profile)) = self.recording_profile_for_session(session_id) else {
            self.recording.finish_action(session_id);
            self.shell.set_status(format!(
                "{} {}",
                t!("recording.startFailed"),
                t!("recording.sessionUnavailable")
            ));
            cx.notify();
            return;
        };
        if let Some(mode) = requested_mode {
            profile.mode = mode;
        }
        self.shell.set_status(t!("recording.starting").to_string());
        let writer = self.recording.writer();
        let job_session_id = session_id.to_string();
        let memory_limit = self.settings.summary().recording_memory_limit_bytes as usize;
        let task = writer.start_async(job_session_id, context, profile, None, memory_limit, false);
        let result_session_id = session_id.to_string();
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|_| "recording writer stopped".to_string())
                .and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                let current_id = this.recording.current_session_id(&result_session_id);
                this.recording.finish_action(&result_session_id);
                match result {
                    Ok(path)
                        if this
                            .session
                            .metadata(&current_id)
                            .is_some_and(|metadata| !metadata.disconnected) =>
                    {
                        this.shell
                            .set_status(format!("{} {path}", t!("recording.started")));
                    }
                    Ok(_) => {
                        this.recording.cleanup_writer_session(&current_id);
                        this.shell
                            .set_status(t!("recording.startCancelled").to_string());
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("{} {error}", t!("recording.startFailed")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn stop_recording_for_session(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.recording.begin_action(session_id, "record") {
            self.shell.set_status(t!("recording.busy").to_string());
            cx.notify();
            return;
        }
        self.shell.set_status(t!("recording.stopping").to_string());
        let writer = self.recording.writer();
        let job_session_id = session_id.to_string();
        let finish_output = self.terminal.recording_output_fence();
        let task = self.blocking_jobs.submit_task("recording-stop", move |_| {
            let fence_error = finish_output().err();
            let mut completion = writer.stop_complete(job_session_id)?;
            if completion.error.is_none() {
                completion.error = fence_error;
            }
            Ok::<_, String>(completion)
        });
        let result_session_id = session_id.to_string();
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                this.recording.finish_action(&result_session_id);
                match result {
                    Ok(completion) => {
                        let path = completion.file_path.display();
                        let message = match completion.error {
                            Some(error) => {
                                t!("recording.stoppedWithError", path = path, error = error)
                                    .to_string()
                            }
                            None => t!("recording.saved", path = path).to_string(),
                        };
                        this.shell.set_status(message);
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("{} {error}", t!("recording.stopFailed")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save_transcript_to_path(
        &mut self,
        session_id: &str,
        path: String,
        unique: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.recording.begin_action(session_id, "save") {
            self.shell.set_status(t!("recording.busy").to_string());
            cx.notify();
            return;
        }
        self.shell.set_status(t!("recording.saving").to_string());
        let writer = self.recording.writer();
        let job_session_id = session_id.to_string();
        let memory_limit = self.settings.summary().recording_memory_limit_bytes as usize;
        let include_io_labels = self.settings.summary().recording_include_io_labels;
        let include_timestamps = self.settings.summary().recording_include_timestamps;
        let finish_output = self.terminal.recording_output_fence();
        let task = self
            .blocking_jobs
            .submit_task("recording-transcript", move |_| {
                finish_output()?;
                writer.save_transcript_with_policy(
                    job_session_id,
                    path,
                    include_io_labels,
                    include_timestamps,
                    memory_limit,
                    unique,
                )
            });
        let result_session_id = session_id.to_string();
        cx.spawn(async move |this, cx| {
            let result = await_blocking_job(task).await.and_then(|result| result);
            let _ = this.update(cx, |this, cx| {
                this.recording.finish_action(&result_session_id);
                match result {
                    Ok(path) => {
                        this.shell
                            .set_status(t!("recording.transcriptSaved", path = path).to_string());
                    }
                    Err(error) => {
                        this.shell
                            .set_status(format!("{} {error}", t!("recording.saveFailed")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::features) fn save_session_transcript_for_session(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.session_info(session_id) else {
            self.shell.set_status(format!(
                "{} {}",
                t!("recording.saveFailed"),
                t!("recording.sessionUnavailable")
            ));
            cx.notify();
            return;
        };
        let path = match session_transcript_file_path(
            self.settings.summary(),
            &session.name,
            OffsetDateTime::now_utc(),
        ) {
            Ok(path) => path,
            Err(error) => {
                self.shell
                    .set_status(format!("{} {error}", t!("recording.saveFailed")));
                cx.notify();
                return;
            }
        };
        self.save_transcript_to_path(session_id, path.display().to_string(), true, cx);
    }

    pub(in crate::features) fn queue_auto_recording(&self, session_id: &str) {
        if !self.effective_recording_auto_start(session_id) {
            return;
        }
        let Some((context, profile)) = self.recording_profile_for_session(session_id) else {
            return;
        };
        // Enqueue before event ownership is handed to the bridge. File I/O stays on the writer.
        drop(self.recording.writer().start_async(
            session_id.into(),
            context,
            profile,
            None,
            self.settings.summary().recording_memory_limit_bytes as usize,
            true,
        ));
    }

    pub(in crate::features) fn cleanup_recording_for_session(&mut self, session_id: &str) {
        self.recording.cleanup_session(session_id);
    }

    pub(in crate::features) fn disconnect_recording_for_session(&self, session_id: &str) {
        self.recording.disconnect_session(session_id);
    }

    pub(in crate::features) fn apply_recording_search(
        &mut self,
        text: String,
        cx: &mut Context<Self>,
    ) {
        self.mark_user_activity();
        self.recording.set_search_draft(text);
        cx.notify();
    }
}

impl NyaTermApp {
    fn recording_profile_for_session(
        &self,
        session_id: &str,
    ) -> Option<(RecordingContext, RecordingProfile)> {
        let metadata = self.session.metadata(session_id)?;
        metadata.launch_config.encoding()?;
        if metadata.disconnected {
            return None;
        }
        let summary = self.settings.summary();
        let (launch_name, protocol, host, port, username) =
            recording_launch_context(&metadata.launch_config);
        let session_name = self
            .session
            .session_info(session_id)
            .map(|session| session.name)
            .unwrap_or(launch_name);
        let saved_connection = metadata
            .source_connection_id
            .as_deref()
            .and_then(|connection_id| {
                self.connection_state
                    .connections()
                    .iter()
                    .find(|connection| connection.id == connection_id)
            });
        let context = RecordingContext {
            session_id: session_id.to_string(),
            session_name,
            connection_id: metadata.source_connection_id.clone(),
            connection_name: saved_connection.map(|connection| connection.name.clone()),
            group_path: saved_connection.and_then(|connection| {
                recording_group_path(
                    self.connection_state.groups(),
                    connection.group_id.as_deref(),
                )
            }),
            protocol,
            host,
            port,
            username,
            started_at: OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc()),
        };
        let connection_recording =
            saved_connection.and_then(|connection| connection.recording.as_ref());
        let profile = RecordingProfile {
            mode: connection_recording
                .and_then(|settings| settings.mode)
                .map(map_recording_mode)
                .unwrap_or_else(|| map_recording_mode(summary.recording_default_mode)),
            base_path: recording_base_path(summary, self.runtime.config_dir()),
            path_template: connection_recording
                .and_then(|settings| settings.path_template.as_ref())
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| summary.recording_path_template.clone()),
            include_timestamps: connection_recording
                .and_then(|settings| settings.include_timestamps)
                .unwrap_or(summary.recording_include_timestamps),
            include_io_labels: summary.recording_include_io_labels,
            include_session_metadata: summary.recording_include_session_metadata,
            rotation: connection_recording
                .and_then(|settings| settings.rotation.as_ref())
                .map(map_recording_rotation)
                .unwrap_or_else(|| map_recording_rotation(&summary.recording_rotation)),
            existing_file_behavior: map_existing_file_behavior(
                summary.recording_existing_file_behavior,
            ),
            include_binary_transfer_payloads: summary.recording_include_binary_transfer_payloads,
            include_input: summary.recording_include_input,
        };
        Some((context, profile))
    }

    fn effective_recording_auto_start(&self, session_id: &str) -> bool {
        let Some(metadata) = self.session.metadata(session_id) else {
            return false;
        };
        metadata
            .source_connection_id
            .as_deref()
            .and_then(|connection_id| {
                self.connection_state
                    .connections()
                    .iter()
                    .find(|connection| connection.id == connection_id)
            })
            .and_then(|connection| connection.recording.as_ref())
            .and_then(|settings| settings.auto_start)
            .unwrap_or(self.settings.summary().recording_auto_start)
    }
}

fn recording_base_path(settings: &AppSettingsSummary, config_dir: &std::path::Path) -> PathBuf {
    if settings.recording_path.trim().is_empty() {
        default_recording_directory().unwrap_or_else(|| config_dir.join("recordings"))
    } else {
        PathBuf::from(settings.recording_path.trim())
    }
}

fn default_recording_directory() -> Option<PathBuf> {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
}

fn session_transcript_file_path(
    settings: &AppSettingsSummary,
    session_name: &str,
    now: OffsetDateTime,
) -> Result<PathBuf, String> {
    let directory = if settings.recording_path.trim().is_empty() {
        default_recording_directory()
            .ok_or_else(|| "failed to resolve Downloads directory".to_string())?
    } else {
        PathBuf::from(settings.recording_path.trim())
    };
    let timestamp = now
        .format(format_description!(
            "[year]-[month]-[day]T[hour]-[minute]-[second]"
        ))
        .map_err(|error| format!("failed to format transcript timestamp: {error}"))?;
    let file_name = format!(
        "session-{}-{timestamp}.log",
        nyaterm_transport::safe_recording_name(session_name)
    );
    Ok(directory.join(file_name))
}

#[cfg(test)]
fn first_available_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("session");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1.. {
        let file_name = extension.map_or_else(
            || format!("{stem}-{index}"),
            |extension| format!("{stem}-{index}.{extension}"),
        );
        let candidate = parent.join(file_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("unbounded suffix search must find an available path")
}

fn recording_group_path(groups: &[Group], group_id: Option<&str>) -> Option<String> {
    let mut current = group_id?;
    let mut names = Vec::new();
    let mut visited = HashSet::new();
    for _ in 0..32 {
        if !visited.insert(current.to_string()) {
            break;
        }
        let group = groups.iter().find(|group| group.id == current)?;
        names.push(group.name.clone());
        let Some(parent) = group.parent_id.as_deref() else {
            break;
        };
        current = parent;
    }
    names.reverse();
    (!names.is_empty()).then(|| names.join("/"))
}

fn recording_launch_context(
    launch_config: &SessionLaunchConfig,
) -> (String, String, Option<String>, Option<u16>, Option<String>) {
    match launch_config {
        SessionLaunchConfig::Local(config) => {
            (config.name.clone(), "local".to_string(), None, None, None)
        }
        SessionLaunchConfig::Ssh(config) => (
            config.name.clone(),
            "ssh".to_string(),
            Some(config.host.clone()),
            Some(config.port),
            Some(config.username.clone()),
        ),
        SessionLaunchConfig::Telnet(config) => (
            config.name.clone(),
            "telnet".to_string(),
            Some(config.host.clone()),
            Some(config.port),
            Some(config.username.clone()),
        ),
        SessionLaunchConfig::Serial(config) => (
            config.name.clone(),
            "serial".to_string(),
            Some(config.port_name.clone()),
            None,
            None,
        ),
        SessionLaunchConfig::Rdp(config) => (
            config.name.clone(),
            "rdp".to_string(),
            Some(config.host.clone()),
            Some(config.port),
            Some(config.username.clone()),
        ),
        SessionLaunchConfig::Vnc(config) => (
            config.name.clone(),
            "vnc".to_string(),
            Some(config.host.clone()),
            Some(config.port),
            None,
        ),
    }
}

fn map_recording_mode(mode: CoreRecordingMode) -> RecordingMode {
    match mode {
        CoreRecordingMode::Raw => RecordingMode::Raw,
        CoreRecordingMode::Transcript => RecordingMode::Transcript,
    }
}

fn map_existing_file_behavior(behavior: CoreExistingFileBehavior) -> TransportExistingFileBehavior {
    match behavior {
        CoreExistingFileBehavior::Append => TransportExistingFileBehavior::Append,
        CoreExistingFileBehavior::Overwrite => TransportExistingFileBehavior::Overwrite,
        CoreExistingFileBehavior::Unique => TransportExistingFileBehavior::Unique,
    }
}

fn map_recording_rotation(
    rotation: &CoreRecordingRotationPolicy,
) -> TransportRecordingRotationPolicy {
    match rotation {
        CoreRecordingRotationPolicy::Daily => TransportRecordingRotationPolicy::Daily,
        CoreRecordingRotationPolicy::Size { max_bytes } => TransportRecordingRotationPolicy::Size {
            max_bytes: *max_bytes,
        },
        CoreRecordingRotationPolicy::Session => TransportRecordingRotationPolicy::Session,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use nyaterm_core::{AppSettingsSummary, Group};
    use time::{Date, Month, PrimitiveDateTime, Time, UtcOffset};

    use super::{first_available_path, recording_group_path, session_transcript_file_path};

    fn group(id: &str, name: &str, parent_id: Option<&str>) -> Group {
        Group {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: parent_id.map(str::to_string),
            ..Group::default()
        }
    }

    #[test]
    fn session_transcript_path_uses_safe_name_and_utc_iso_timestamp() {
        let settings = AppSettingsSummary {
            recording_path: "/tmp/nyaterm-recordings".to_string(),
            ..AppSettingsSummary::default()
        };
        let now = PrimitiveDateTime::new(
            Date::from_calendar_date(2026, Month::August, 15).expect("valid date"),
            Time::from_hms(7, 8, 9).expect("valid time"),
        )
        .assume_offset(UtcOffset::UTC);

        let path =
            session_transcript_file_path(&settings, "prod / shell", now).expect("transcript path");

        // Compare against a joined `PathBuf` rather than a `/`-separated literal so
        // the expectation matches the platform separator `join` emits.
        let expected = PathBuf::from("/tmp/nyaterm-recordings")
            .join("session-prod_shell-2026-08-15T07-08-09.log");
        assert_eq!(path, expected);
    }

    #[test]
    fn first_available_path_never_overwrites_an_existing_log() {
        let directory = nyaterm_core::test_support::TestTempDir::new("nyaterm-recording-path");
        fs::create_dir_all(directory.path()).expect("create test directory");
        let path = directory.join("session-demo.log");
        fs::write(&path, b"existing").expect("write existing log");
        fs::write(directory.join("session-demo-1.log"), b"existing")
            .expect("write first collision");

        assert_eq!(
            first_available_path(&path),
            directory.join("session-demo-2.log")
        );
    }

    #[test]
    fn recording_group_path_is_nested_cycle_safe_and_bounded() {
        let nested = vec![
            group("root", "Production", None),
            group("region", "Shanghai", Some("root")),
            group("host", "Databases", Some("region")),
        ];
        assert_eq!(
            recording_group_path(&nested, Some("host")).as_deref(),
            Some("Production/Shanghai/Databases")
        );

        let cycle = vec![group("a", "A", Some("b")), group("b", "B", Some("a"))];
        assert_eq!(
            recording_group_path(&cycle, Some("a")).as_deref(),
            Some("B/A")
        );

        let broken = vec![group("child", "Child", Some("missing"))];
        assert_eq!(recording_group_path(&broken, Some("child")), None);

        let deep = (0..40)
            .map(|index| {
                group(
                    &format!("g{index}"),
                    &format!("G{index}"),
                    (index > 0).then(|| format!("g{}", index - 1)).as_deref(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            recording_group_path(&deep, Some("g39"))
                .expect("bounded group path")
                .split('/')
                .count(),
            32
        );
    }
    #[test]
    fn recording_auto_start_honors_connection_overrides_batches_and_reconnects() {
        use crate::features::test_support::app_with_visible_local_session;
        use crate::models::SessionLaunchConfig;
        use gpui::{AppContext as _, TestAppContext};
        use nyaterm_core::{ConnectionRecordingSettings, SavedConnection};

        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-auto-recording-app");
        let mut cx = TestAppContext::single();
        let _test_app_cleanup = crate::test_support::TestAppCleanup::new(&cx);
        let app = app_with_visible_local_session(&mut cx, root.path(), "fixture");
        cx.update_entity(&app, |app, _| {
            let mut summary = app.settings.summary().clone();
            summary.recording_auto_start = false;
            summary.recording_path = root.join("logs").to_string_lossy().into_owned();
            summary.recording_path_template = "{session}.log".into();
            summary.recording_existing_file_behavior = nyaterm_core::ExistingFileBehavior::Unique;
            app.settings.replace_summary(summary);
            let mut connection: SavedConnection = serde_json::from_value(serde_json::json!({
                "id": "enabled", "name": "synthetic", "type": "ssh", "host": "example.test"
            }))
            .unwrap();
            connection.recording = Some(ConnectionRecordingSettings {
                auto_start: Some(true),
                ..Default::default()
            });
            let mut disabled = connection.clone();
            disabled.id = "disabled".into();
            disabled.recording.as_mut().unwrap().auto_start = Some(false);
            app.connection_state
                .replace_loaded(vec![connection, disabled], vec![]);
            let base = app.session.metadata("fixture").unwrap().clone();
            for id in ["first", "second"] {
                let mut metadata = base.clone();
                metadata.source_connection_id = Some("enabled".into());
                if let SessionLaunchConfig::Local(config) = &mut metadata.launch_config {
                    config.name = id.into();
                }
                app.register_session_for_start(id, metadata, None, None);
                app.recording
                    .writer()
                    .write_output(id, format!("{id} output\n"));
            }
            let mut metadata = base;
            metadata.source_connection_id = Some("disabled".into());
            let mut summary = app.settings.summary().clone();
            summary.recording_auto_start = true;
            app.settings.replace_summary(summary);
            app.register_session_for_start("disabled", metadata, None, None);
            assert!(!app.effective_recording_auto_start("disabled"));
            let metadata = app.session.metadata("first").unwrap().clone();
            app.recording.rekey_session("first", "reconnected");
            app.register_session_for_reconnect("reconnected", metadata);
            let completion = app
                .recording
                .writer()
                .stop_complete("reconnected".into())
                .unwrap();
            assert!(completion.error.is_none());
            assert!(
                fs::read_to_string(&completion.file_path)
                    .unwrap()
                    .contains("first output")
            );
            app.recording
                .writer()
                .stop_complete("second".into())
                .unwrap();
            assert!(
                app.recording
                    .writer()
                    .stop_complete("disabled".into())
                    .is_err()
            );
            assert_eq!(fs::read_dir(root.join("logs")).unwrap().count(), 2);
        });
    }
}

#[cfg(test)]
mod manual_tests;

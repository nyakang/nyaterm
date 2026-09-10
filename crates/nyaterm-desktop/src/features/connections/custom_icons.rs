use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui::{AppContext as _, Context, PathPromptOptions, RenderImage, Window};
use nyaterm_core::models::sessions::ConnectionCustomIcon;
use nyaterm_store::{StoreDomain, store_request};

use crate::features::{NyaTermApp, runtime_jobs::await_blocking_job};

#[derive(Default)]
pub(in crate::features) struct CustomIconState {
    pub records: Vec<ConnectionCustomIcon>,
    pub images: Arc<HashMap<String, Arc<RenderImage>>>,
    generation: u64,
}

const MAX_ICON_BYTES: usize = 2 * 1024 * 1024;

fn decode_icon(record: &ConnectionCustomIcon) -> Result<Arc<RenderImage>, String> {
    let (_, encoded) = record
        .data_url
        .split_once(',')
        .ok_or("Invalid image data URL")?;
    if encoded.len() > MAX_ICON_BYTES * 4 / 3 + 4 {
        return Err("Icon exceeds 2 MiB".into());
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| "Invalid image encoding")?;
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Invalid image format")?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|_| "Invalid or oversized icon")?
        .thumbnail(128, 128);
    let mut pixels = image.to_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
}

impl NyaTermApp {
    fn invalidate_connection_icon_views(&mut self, cx: &mut Context<Self>) {
        self.flush_connection_panel_snapshot(cx);
        cx.notify();
    }

    pub(in crate::features) fn update_custom_icons(
        &mut self,
        records: Vec<ConnectionCustomIcon>,
        cx: &mut Context<Self>,
    ) {
        if self.connection_state.custom_icons.records == records {
            return;
        }
        let state = &mut self.connection_state.custom_icons;
        state.records = records.clone();
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        cx.spawn(async move |this, cx| {
            let images = cx
                .background_spawn(async move {
                    records
                        .into_iter()
                        .filter_map(|record| {
                            decode_icon(&record).ok().map(|image| (record.id, image))
                        })
                        .collect::<HashMap<_, _>>()
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.connection_state.custom_icons.generation != generation {
                    return;
                }
                app.connection_state.custom_icons.images = Arc::new(images);
                app.invalidate_connection_icon_views(cx);
            });
        })
        .detach();
    }

    pub(in crate::features) fn import_connection_custom_icon(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origin_window = Some(window.window_handle());
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let jobs = self.blocking_jobs.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picker.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let result =
                await_blocking_job(jobs.submit_task("connection-icon-import", move |_| {
                    use std::io::Read as _;
                    let file = std::fs::File::open(&path).map_err(|_| "Cannot open icon")?;
                    let mut bytes = Vec::new();
                    file.take((MAX_ICON_BYTES + 1) as u64)
                        .read_to_end(&mut bytes)
                        .map_err(|_| "Cannot read icon")?;
                    if bytes.len() > MAX_ICON_BYTES {
                        return Err("Icon exceeds 2 MiB".to_string());
                    }
                    let format =
                        image::guess_format(&bytes).map_err(|_| "Unsupported icon format")?;
                    let mime = match format {
                        image::ImageFormat::Png => "png",
                        image::ImageFormat::Jpeg => "jpeg",
                        image::ImageFormat::Gif => "gif",
                        image::ImageFormat::Bmp => "bmp",
                        image::ImageFormat::WebP => "webp",
                        _ => return Err("Unsupported icon format".into()),
                    };
                    let data_url = format!("data:image/{mime};base64,{}", STANDARD.encode(bytes));
                    let name = path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Icon")
                        .to_string();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let record = ConnectionCustomIcon::from_legacy_data_url(&data_url, name, now)
                        .ok_or("Invalid icon")?;
                    decode_icon(&record)?;
                    Ok::<_, String>(record)
                }))
                .await
                .and_then(|result| result);
            let _ = this.update(cx, |app, cx| match result {
                Ok(record) => {
                    let id = record.id.clone();
                    app.submit_store_request(
                        0,
                        store_request(StoreDomain::Connections, move |store| {
                            store.save_connection_custom_icon(&record)?;
                            store.load_sessions()
                        }),
                        move |app, event, cx| match event.outcome {
                            Ok(sessions) => {
                                app.apply_loaded_sessions(sessions, cx);
                                app.set_connection_editor_icon(Some(&id), cx);
                            }
                            Err(_) => app.notify_operation_at(
                                origin_window,
                                "icon-save",
                                nyaterm_ui::notification::NyaNotificationKind::Error,
                                rust_i18n::t!("dialog.customIconFailed").to_string(),
                                cx,
                            ),
                        },
                        cx,
                    );
                }
                Err(_) => app.notify_operation_at(
                    origin_window,
                    "icon-import",
                    nyaterm_ui::notification::NyaNotificationKind::Error,
                    rust_i18n::t!("dialog.customIconFailed").to_string(),
                    cx,
                ),
            });
        })
        .detach();
    }

    pub(in crate::features) fn delete_connection_custom_icon(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.submit_store_request(
            0,
            store_request(StoreDomain::Connections, move |store| {
                store.delete_connection_custom_icon(&id)?;
                store.load_sessions()
            }),
            |app, event, cx| match event.outcome {
                Ok(sessions) => {
                    app.apply_loaded_sessions(sessions, cx);
                    app.set_connection_editor_icon(None, cx);
                }
                Err(_) => app.notify_operation(
                    "icon-delete",
                    nyaterm_ui::notification::NyaNotificationKind::Error,
                    rust_i18n::t!("dialog.customIconFailed").to_string(),
                    cx,
                ),
            },
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::decode_icon;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use nyaterm_core::models::sessions::ConnectionCustomIcon;
    #[test]
    fn imported_image_is_decoded_and_invalid_payload_is_rejected() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../nyaterm-app/resources/icons/32x32.png"
        ));
        let url = format!("data:image/png;base64,{}", STANDARD.encode(bytes));
        let mut record =
            ConnectionCustomIcon::from_legacy_data_url(&url, "fixture".into(), 1).unwrap();
        assert!(decode_icon(&record).is_ok());
        record.data_url = "data:image/png;base64,AA==".into();
        assert!(decode_icon(&record).is_err());
    }
}

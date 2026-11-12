//! GPUI conversion only; callers prepare BGRA pixels on background workers.

use std::sync::Arc;

use gpui::RenderImage;

pub(crate) fn render_image_from_bgra(
    width: u32,
    height: u32,
    bgra: &[u8],
) -> Result<Arc<RenderImage>, String> {
    let buffer = image::RgbaImage::from_raw(width, height, bgra.to_vec())
        .ok_or_else(|| "inconsistent pixel buffer".to_string())?;
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

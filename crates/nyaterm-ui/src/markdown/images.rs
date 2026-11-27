use std::sync::{Arc, OnceLock};

use gpui::{Image, ImageFormat, ImageSource, SharedUri};
use gpui_base::text::TextView;

/// Resource policy for a Markdown surface. Redirects and caching are handled
/// by the application's existing image loader, not by this source filter.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum NyaMarkdownImagePolicy {
    #[default]
    Existing,
    HttpsOnly,
}

impl NyaMarkdownImagePolicy {
    pub(crate) fn apply(self, view: TextView) -> TextView {
        match self {
            Self::Existing => view,
            Self::HttpsOnly => view.image_source(https_image_source),
        }
    }
}

fn https_image_source(uri: &SharedUri) -> ImageSource {
    if let Some(url) = allowed_image_url(uri.as_ref()) {
        // Forward the parsed, normalized URL rather than asking another parser
        // to interpret the original string (backslashes, whitespace, etc.).
        return ImageSource::from(url.to_string());
    }
    static PLACEHOLDER: OnceLock<Arc<Image>> = OnceLock::new();
    ImageSource::Image(
        PLACEHOLDER
            .get_or_init(|| {
                Arc::new(Image::from_bytes(
                    ImageFormat::Svg,
                    br##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="24" viewBox="0 0 32 24"><rect x="1" y="1" width="30" height="22" rx="3" fill="none" stroke="#808080"/><path d="m5 18 7-8 5 5 4-4 6 7M5 3l22 18" fill="none" stroke="#808080" stroke-width="2"/></svg>"##.to_vec(),
                ))
            })
            .clone(),
    )
}

fn allowed_image_url(source: &str) -> Option<url::Url> {
    let url = url::Url::parse(source).ok()?;
    (url.scheme() == "https"
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

#[cfg(test)]
mod tests {
    use gpui::{ImageSource, Resource, SharedUri};

    use super::https_image_source;

    #[test]
    fn absolute_https_images_use_normalized_network_sources() {
        for source in [
            "https://example.test/a.png?q=1#image",
            "HTTPS://EXAMPLE.TEST/a.png",
        ] {
            let uri: SharedUri = source.to_string().into();
            let ImageSource::Resource(Resource::Uri(url)) = https_image_source(&uri) else {
                panic!("expected HTTPS resource");
            };
            assert_eq!(url.as_ref(), url::Url::parse(source).unwrap().as_str());
        }
    }

    #[test]
    fn other_schemes_paths_and_credentials_never_reach_the_resource_loader() {
        for source in [
            "http://example.test/a.png",
            "//example.test/a.png",
            "../a.png",
            "/a.png",
            r"C:\a.png",
            "file:///a.png",
            "data:image/png;base64,AA==",
            "javascript:alert(1)",
            "ftp://example.test/a.png",
            "",
            "https://",
            "https://user@example.test/a.png",
            "https://:secret@example.test/a.png",
            "https://user:secret@example.test/a.png",
        ] {
            let uri: SharedUri = source.to_string().into();
            assert!(matches!(https_image_source(&uri), ImageSource::Image(_)));
        }
    }
}

use gpui::{
    AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, TestAppContext, Window, div, px,
};

use crate::NyaScrollable as _;
use crate::markdown::{NyaMarkdown, NyaMarkdownImagePolicy};
use crate::root::nya_root;
use crate::text::{NyaTextFormat, NyaTextState, NyaTextStyle};
use crate::theme::theme_palette;

const DOCUMENT: &str = "# 标题\n\nHello **bold** and `code` and [link](https://example.invalid).\n\n- bullet\n- [x] done\n\n1. numbered\n\n> quote line 1\n> quote line 2\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n| short |\n\n```rust\nlet x = 1;\n```\n\n---";

struct PreviewFixture {
    text: NyaTextState,
    visible: bool,
    commits: usize,
    _subscription: Subscription,
}

impl PreviewFixture {
    fn new(source: &str, cx: &mut Context<Self>) -> Self {
        let text = NyaTextState::new(NyaTextFormat::Markdown, source, cx);
        let subscription = text.observe_layout(cx, |this, cx| {
            this.commits += 1;
            cx.notify();
        });
        Self {
            text,
            visible: true,
            commits: 0,
            _subscription: subscription,
        }
    }
}

impl Render for PreviewFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut view = div().w(px(320.)).h(px(240.)).overflow_y_scrollbar();
        if self.visible {
            view = view.child(
                div()
                    .debug_selector(|| "preview-document".into())
                    .w_full()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .child(
                        NyaMarkdown::from_state(&self.text)
                            .style(NyaTextStyle::document_preview(theme_palette("github-dark")))
                            .image_policy(NyaMarkdownImagePolicy::HttpsOnly),
                    ),
            );
        }
        view
    }
}

#[gpui::test]
fn document_preview_renders_mixed_markdown_and_copies_readable_content(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let fixture = cx.new(|cx| PreviewFixture::new(DOCUMENT, cx));
    let (_, vcx) = cx.add_window_view(|window, cx| nya_root(fixture.clone(), window, cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    vcx.run_until_parked();
    let bounds = vcx
        .debug_bounds("preview-document")
        .expect("document laid out");
    assert_eq!(bounds.size.width, px(320.));
    assert!(bounds.size.height > px(100.));
    vcx.update(|_, cx| {
        let text = fixture.read(cx).text.clone();
        text.select_all(cx);
        let copied = text.selected_text(cx);
        for expected in [
            "标题",
            "Hello bold",
            "bullet",
            "done",
            "numbered",
            "quote line 1",
            "short",
            "let x = 1;",
        ] {
            assert!(
                copied.contains(expected),
                "missing {expected:?} in copied document"
            );
        }
    });
}

#[gpui::test]
fn preview_unmount_and_remount_keep_state_without_reparsing(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let fixture = cx.new(|cx| PreviewFixture::new(DOCUMENT, cx));
    let (_, vcx) = cx.add_window_view(|window, cx| nya_root(fixture.clone(), window, cx));
    vcx.run_until_parked();
    let commits = fixture.read_with(vcx, |view, _| view.commits);
    for visible in [false, true, false, true] {
        fixture.update(vcx, |view, cx| {
            view.visible = visible;
            cx.notify();
        });
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        vcx.run_until_parked();
    }
    fixture.read_with(vcx, |view, _| {
        assert!(view.text.matches_source(DOCUMENT));
        assert_eq!(view.commits, commits);
    });
}

#[gpui::test]
fn large_preview_replacements_parse_asynchronously_and_notify_layout(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let fixture = cx.new(|cx| PreviewFixture::new("old document", cx));
    cx.run_until_parked();
    for bytes in [100_000, 1_000_000] {
        let source = format!(
            "# Document {bytes}\n\n{}",
            "paragraph 中文\n\n".repeat(bytes / 18)
        );
        let commits = fixture.read_with(cx, |view, _| view.commits);
        fixture.update(cx, |view, cx| {
            view.text.select_all(cx);
            view.text.set_text(&source, cx);
            assert!(view.text.matches_source(&source));
            assert_eq!(view.text.selected_text(cx), "");
            assert_eq!(view.commits, commits);
        });
        cx.run_until_parked();
        fixture.update(cx, |view, cx| {
            assert!(view.commits > commits);
            view.text.select_all(cx);
            assert!(
                view.text
                    .selected_text(cx)
                    .starts_with(&format!("Document {bytes}"))
            );
        });
    }
}

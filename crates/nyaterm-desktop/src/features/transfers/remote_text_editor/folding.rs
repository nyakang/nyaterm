use super::RemoteTextEditor;
use gpui::{AppContext as _, Context};

impl RemoteTextEditor {
    pub(super) fn fold_language_for_path(path: &str) -> String {
        match path.rsplit('.').next().unwrap_or("") {
            "json" => "json",
            "yaml" | "yml" => "yaml",
            "toml" => "toml",
            "rs" => "rust",
            "py" => "python",
            "js" | "jsx" => "javascript",
            "ts" | "tsx" => "typescript",
            "sh" | "bash" => "bash",
            "go" => "go",
            "c" | "h" => "c",
            "cpp" | "hpp" => "cpp",
            "css" => "css",
            "html" | "htm" => "html",
            "xml" | "svg" => "xml",
            "md" => "markdown",
            _ => "plain",
        }
        .to_string()
    }

    pub(super) fn schedule_fold_parse(&mut self, cx: &mut Context<Self>) {
        self.fold_generation = self.fold_generation.wrapping_add(1);
        let generation = self.fold_generation;
        let content = self.document.content.clone();
        let language = self.fold_language.clone();
        self.fold_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(60))
                .await;
            let ranges = cx
                .background_spawn(async move {
                    nyaterm_ui::document_syntax::document_fold_ranges(&content, &language)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.fold_generation != generation {
                    return;
                }
                this.folds.retain(|range| ranges.contains(range));
                this.fold_candidates = ranges;
                cx.notify();
            });
        }));
    }

    pub(super) fn fold_at_cursor(&mut self, cx: &mut Context<Self>) {
        let cursor = self.document.head;
        let line_end = self.document.content[cursor..]
            .find('\n')
            .map_or(self.document.content.len(), |end| cursor + end + 1);
        if let Some(range) = self
            .fold_candidates
            .iter()
            .filter(|range| range.start <= line_end && range.end > cursor)
            .min_by_key(|range| range.len())
            .cloned()
        {
            self.document.anchor = range.start;
            self.document.head = range.start;
            self.document.additional_selections.clear();
            if self.folds.contains(&range) {
                self.folds.retain(|fold| fold != &range);
            } else {
                self.folds.push(range);
            }
            self.last_layout = None;
            cx.notify();
        }
    }
}

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, StyleRefinement, TextStyleRefinement,
    Window, div, point, prelude::*, px, size,
};

use gpui_base::StyledExt as _;

use super::{NyaTextScope, NyaTextState, NyaTextStyle};

pub(super) struct TextPresentation {
    style: NyaTextStyle,
    typography: TextStyleRefinement,
}

impl TextPresentation {
    pub(super) fn new(style: NyaTextStyle, window: &Window) -> Self {
        let text = window.text_style();
        Self {
            style,
            typography: TextStyleRefinement {
                color: Some(text.color),
                font_family: Some(text.font_family.clone()),
                font_features: Some(text.font_features.clone()),
                font_fallbacks: text.font_fallbacks.clone(),
                font_size: Some(text.font_size),
                line_height: Some(text.line_height),
                font_weight: Some(text.font_weight),
                font_style: Some(text.font_style),
                ..Default::default()
            },
        }
    }

    fn element(&self, state: &NyaTextState, cx: &mut App) -> AnyElement {
        state.prepare_retained_selection(self.style.0.selection(), cx);
        let view = self.style.apply(state.view());
        let style = StyleRefinement {
            text: self.typography.clone(),
            ..Default::default()
        };
        div()
            .w_full()
            .refine_style(&style)
            .child(view)
            .into_any_element()
    }
}

/// These are real, clipped TextViews, so native hit testing and selection
/// projections continue to own the endpoint byte offsets through reflow.
/// At most two offscreen views are retained; the transcript stays virtual.
pub(super) struct SelectionRetention {
    scope: NyaTextScope,
    child: AnyElement,
}

impl SelectionRetention {
    pub(super) fn new(scope: NyaTextScope, child: impl IntoElement) -> Self {
        Self {
            scope,
            child: child.into_any_element(),
        }
    }
}

impl IntoElement for SelectionRetention {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

pub(super) struct RetainedEndpoints {
    before: Vec<AnyElement>,
    after: Vec<AnyElement>,
}

impl Element for SelectionRetention {
    type RequestLayoutState = ();
    type PrepaintState = RetainedEndpoints;

    fn id(&self) -> Option<ElementId> {
        Some("conversation-selection-retention".into())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> RetainedEndpoints {
        let retained_selection = self
            .scope
            .0
            .borrow()
            .iter()
            .any(NyaTextState::has_retained_selection);
        if !gpui_base::TextSelection::has_selection(window, cx) && !retained_selection {
            self.scope.1.set(None);
            self.child.prepaint(window, cx);
            return RetainedEndpoints {
                before: Vec::new(),
                after: Vec::new(),
            };
        }
        for state in self.scope.0.borrow().iter() {
            state.painted.set(false);
        }
        self.child.prepaint(window, cx);
        let members = self.scope.0.borrow();
        let mut retained = RetainedEndpoints {
            before: Vec::new(),
            after: Vec::new(),
        };
        let selected: Vec<_> = members
            .iter()
            .enumerate()
            .filter(|(_, state)| {
                !state.selected_text(cx).is_empty() || state.inner.read(cx).is_selecting()
            })
            .map(|(index, _)| index)
            .collect();
        let dragging = members
            .iter()
            .any(|state| state.inner.read(cx).is_selecting());
        if !gpui_base::TextSelection::has_selection(window, cx) && !retained_selection {
            self.scope.1.set(None);
        }
        if (dragging || self.scope.1.get().is_none())
            && let (Some(first), Some(last)) = (selected.first(), selected.last())
        {
            self.scope.1.set(Some((
                members[*first].inner.entity_id(),
                members[*last].inner.entity_id(),
            )));
        }
        let Some((first_id, last_id)) = self.scope.1.get() else {
            return retained;
        };
        let Some(first) = members
            .iter()
            .position(|state| state.inner.entity_id() == first_id)
        else {
            return retained;
        };
        let Some(last) = members
            .iter()
            .position(|state| state.inner.entity_id() == last_id)
        else {
            return retained;
        };
        let first_painted = members
            .iter()
            .position(|state| state.painted.get())
            .unwrap_or(members.len());
        for index in [Some(first), (first != last).then_some(last)]
            .into_iter()
            .flatten()
        {
            let state = &members[index];
            if state.painted.get() {
                continue;
            }
            let presentation = state.presentation.borrow();
            let Some(presentation) = presentation.as_ref() else {
                continue;
            };
            let previous = state.inner.read(cx).bounds();
            if previous.size.width <= px(0.) {
                continue;
            }
            let before = index < first_painted;
            let mut element = presentation.element(state, cx);
            let measured = element.layout_as_root(
                size(
                    AvailableSpace::Definite(previous.size.width),
                    AvailableSpace::MinContent,
                ),
                window,
                cx,
            );
            let origin: Point<Pixels> = point(
                previous.left(),
                if before {
                    bounds.top() - measured.height - px(1.)
                } else {
                    bounds.bottom() + px(1.)
                },
            );
            window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                element.prepaint_at(origin, window, cx);
            });
            if before {
                retained.before.push(element);
            } else {
                retained.after.push(element);
            }
        }
        retained
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        retained: &mut RetainedEndpoints,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for element in &mut retained.before {
                element.paint(window, cx);
            }
            self.child.paint(window, cx);
            for element in &mut retained.after {
                element.paint(window, cx);
            }
        });
    }
}

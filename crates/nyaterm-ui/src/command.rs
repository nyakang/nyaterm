use std::rc::Rc;

use gpui::{
    Action, AnyElement, App, AppContext as _, Context, DefiniteLength, Entity, FocusHandle,
    Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Window, div, prelude::FluentBuilder as _,
};
use gpui_base::actions::Cancel;
use gpui_kit::component::{
    Disableable as _, IndexPath,
    command::{Command, CommandItem, CommandState},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NyaCommandIndex {
    pub section: usize,
    pub row: usize,
}

impl From<IndexPath> for NyaCommandIndex {
    fn from(index: IndexPath) -> Self {
        Self {
            section: index.section,
            row: index.row,
        }
    }
}

impl From<NyaCommandIndex> for IndexPath {
    fn from(index: NyaCommandIndex) -> Self {
        IndexPath::new(index.row).section(index.section)
    }
}

pub struct NyaCommandState {
    inner: Entity<CommandState>,
}

impl NyaCommandState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            inner: cx.new(|cx| CommandState::new(window, cx)),
        }
    }

    pub fn query(&self, cx: &App) -> String {
        self.inner.read(cx).query(cx).to_string()
    }

    pub fn selected_index(&self, cx: &App) -> Option<NyaCommandIndex> {
        self.inner.read(cx).selected_index().map(Into::into)
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.inner.read(cx).focus_handle(cx)
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle(cx).focus(window, cx);
    }

    pub fn set_query(
        &mut self,
        query: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.inner
            .update(cx, |state, cx| state.set_query(query, window, cx));
    }

    pub fn set_selected_index(
        &mut self,
        index: Option<NyaCommandIndex>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.inner.update(cx, |state, cx| {
            state.set_selected_index(index.map(Into::into), window, cx)
        });
    }
}

#[derive(Clone)]
pub struct NyaCommandItem {
    inner: CommandItem,
}

impl NyaCommandItem {
    pub fn new() -> Self {
        Self {
            inner: CommandItem::new(),
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<SharedString>,
    {
        self.inner = self.inner.keywords(keywords);
        self
    }

    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        self.inner = self.inner.action(action);
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.inner = self.inner.checked(checked);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.inner = self.inner.disabled(disabled);
        self
    }

    pub fn child<F, E>(mut self, builder: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.inner = self.inner.child(builder);
        self
    }
}

impl Default for NyaCommandItem {
    fn default() -> Self {
        Self::new()
    }
}

type NyaCommandSlot = dyn Fn(&mut Window, &mut App) -> AnyElement;
type NyaCommandQueryHandler = dyn Fn(&str, &mut Window, &mut App);
type NyaCommandIndexHandler = dyn Fn(NyaCommandIndex, &mut Window, &mut App);
type NyaCommandCancelHandler = dyn Fn(&mut Window, &mut App);

#[derive(IntoElement)]
pub struct NyaCommand {
    state: Entity<NyaCommandState>,
    items: Vec<NyaCommandItem>,
    searchable: bool,
    filterable: bool,
    placeholder: Option<SharedString>,
    max_h: Option<DefiniteLength>,
    bordered: bool,
    empty: Option<Rc<NyaCommandSlot>>,
    footer: Option<Rc<NyaCommandSlot>>,
    on_query: Option<Rc<NyaCommandQueryHandler>>,
    on_select: Option<Rc<NyaCommandIndexHandler>>,
    on_confirm: Option<Rc<NyaCommandIndexHandler>>,
    on_cancel: Option<Rc<NyaCommandCancelHandler>>,
    consume_cancel: bool,
}

impl NyaCommand {
    pub fn new(state: &Entity<NyaCommandState>) -> Self {
        Self {
            state: state.clone(),
            items: Vec::new(),
            searchable: true,
            filterable: true,
            placeholder: None,
            max_h: None,
            bordered: true,
            empty: None,
            footer: None,
            on_query: None,
            on_select: None,
            on_confirm: None,
            on_cancel: None,
            consume_cancel: false,
        }
    }

    pub fn item(mut self, item: NyaCommandItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = NyaCommandItem>) -> Self {
        self.items.extend(items);
        self
    }

    pub fn searchable(mut self, searchable: bool) -> Self {
        self.searchable = searchable;
        self
    }

    pub fn filterable(mut self, filterable: bool) -> Self {
        self.filterable = filterable;
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn max_h(mut self, max_h: impl Into<DefiniteLength>) -> Self {
        self.max_h = Some(max_h.into());
        self
    }

    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    pub fn empty<F, E>(mut self, render: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.empty = Some(Rc::new(move |window, cx| {
            render(window, cx).into_any_element()
        }));
        self
    }

    pub fn footer<F, E>(mut self, render: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        self.footer = Some(Rc::new(move |window, cx| {
            render(window, cx).into_any_element()
        }));
        self
    }

    pub fn on_query(mut self, handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        self.on_query = Some(Rc::new(handler));
        self
    }

    pub fn on_select(
        mut self,
        handler: impl Fn(NyaCommandIndex, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn on_confirm(
        mut self,
        handler: impl Fn(NyaCommandIndex, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_confirm = Some(Rc::new(handler));
        self
    }

    pub fn on_cancel(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }

    pub fn consume_cancel(mut self, consume_cancel: bool) -> Self {
        self.consume_cancel = consume_cancel;
        self
    }
}

impl RenderOnce for NyaCommand {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let inner_state = self.state.read(cx).inner.clone();
        let mut command = Command::new(&inner_state)
            .items(self.items.into_iter().map(|item| item.inner))
            .searchable(self.searchable)
            .filterable(self.filterable)
            .bordered(self.bordered);

        if let Some(placeholder) = self.placeholder {
            command = command.placeholder(placeholder);
        }
        if let Some(max_h) = self.max_h {
            command = command.max_h(max_h);
        }
        if let Some(empty) = self.empty {
            command = command.empty(move |_, window, cx| empty(window, cx));
        }
        if let Some(footer) = self.footer {
            command = command.footer(move |_, window, cx| footer(window, cx));
        }
        if let Some(on_query) = self.on_query {
            command = command.on_query(move |query, window, cx| on_query(query, window, cx));
        }
        if let Some(on_select) = self.on_select {
            command =
                command.on_select(move |index, window, cx| on_select(index.into(), window, cx));
        }
        if let Some(on_confirm) = self.on_confirm {
            command =
                command.on_confirm(move |index, window, cx| on_confirm(index.into(), window, cx));
        }
        if let Some(on_cancel) = self.on_cancel {
            command = command.on_cancel(move |window, cx| on_cancel(window, cx));
        }

        div()
            .when(self.consume_cancel, |this| {
                this.on_action(|_: &Cancel, _, cx| cx.stop_propagation())
            })
            .child(command)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use gpui::{
        AppContext as _, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        TestAppContext,
    };
    use gpui_base::actions::Cancel;

    use super::{NyaCommand, NyaCommandIndex, NyaCommandItem, NyaCommandState};

    struct CommandFixture {
        state: gpui::Entity<NyaCommandState>,
    }

    impl Render for CommandFixture {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            NyaCommand::new(&self.state).items([
                NyaCommandItem::new().label("SSH server"),
                NyaCommandItem::new().label("Local terminal"),
            ])
        }
    }

    struct CommandInteractionFixture {
        state: gpui::Entity<NyaCommandState>,
        cancel_count: Rc<Cell<usize>>,
        propagated_cancel_count: Rc<Cell<usize>>,
        raw_keydown_count: Rc<Cell<usize>>,
        confirmed: Rc<RefCell<Vec<NyaCommandIndex>>>,
    }

    impl Render for CommandInteractionFixture {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let cancel_count = Rc::clone(&self.cancel_count);
            let propagated_cancel_count = Rc::clone(&self.propagated_cancel_count);
            let raw_keydown_count = Rc::clone(&self.raw_keydown_count);
            let confirmed = Rc::clone(&self.confirmed);
            gpui::div()
                .on_key_down(move |_, _, _| raw_keydown_count.set(raw_keydown_count.get() + 1))
                .on_action(move |_: &Cancel, _, _| {
                    propagated_cancel_count.set(propagated_cancel_count.get() + 1);
                })
                .child(
                    NyaCommand::new(&self.state)
                        .items([
                            NyaCommandItem::new().label("SSH server"),
                            NyaCommandItem::new().label("Local terminal"),
                        ])
                        .on_confirm(move |index, _, _| confirmed.borrow_mut().push(index))
                        .on_cancel(move |_, _| cancel_count.set(cancel_count.get() + 1))
                        .consume_cancel(true),
                )
        }
    }

    #[gpui::test]
    fn command_state_exposes_query_and_selection_without_component_types(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (fixture, cx) = cx.add_window_view(|window, cx| CommandFixture {
            state: cx.new(|cx| NyaCommandState::new(window, cx)),
        });
        let state = fixture.read_with(cx, |fixture, _| fixture.state.clone());

        cx.update(|window, cx| {
            _ = window.draw(cx);
            assert_eq!(
                state.read(cx).selected_index(cx),
                Some(NyaCommandIndex { section: 0, row: 0 })
            );
            state.update(cx, |state, cx| state.set_query("local", window, cx));
        });
        cx.run_until_parked();

        assert_eq!(state.read_with(cx, |state, cx| state.query(cx)), "local");
        assert_eq!(
            state.read_with(cx, |state, cx| state.selected_index(cx)),
            Some(NyaCommandIndex { section: 0, row: 1 })
        );

        cx.update(|window, cx| {
            state.update(cx, |state, cx| {
                state.set_query("", window, cx);
                state.focus(window, cx);
            });
        });
        cx.simulate_keystrokes("s");
        cx.run_until_parked();
        assert_eq!(state.read_with(cx, |state, cx| state.query(cx)), "s");
    }

    #[gpui::test]
    fn escape_clears_then_cancels_without_leaking_and_enter_confirms(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let cancel_count = Rc::new(Cell::new(0));
        let propagated_cancel_count = Rc::new(Cell::new(0));
        let raw_keydown_count = Rc::new(Cell::new(0));
        let confirmed = Rc::new(RefCell::new(Vec::new()));
        let (fixture, cx) = cx.add_window_view({
            let cancel_count = Rc::clone(&cancel_count);
            let propagated_cancel_count = Rc::clone(&propagated_cancel_count);
            let raw_keydown_count = Rc::clone(&raw_keydown_count);
            let confirmed = Rc::clone(&confirmed);
            move |window, cx| CommandInteractionFixture {
                state: cx.new(|cx| NyaCommandState::new(window, cx)),
                cancel_count,
                propagated_cancel_count,
                raw_keydown_count,
                confirmed,
            }
        });
        let state = fixture.read_with(cx, |fixture, _| fixture.state.clone());

        cx.update(|window, cx| {
            _ = window.draw(cx);
            state.update(cx, |state, cx| {
                state.set_query("ssh", window, cx);
                state.focus(window, cx);
            });
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(state.read_with(cx, |state, cx| state.query(cx)), "");
        assert_eq!(cancel_count.get(), 0);

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(cancel_count.get(), 1);
        assert_eq!(propagated_cancel_count.get(), 0);

        cx.simulate_keystrokes("down enter");
        cx.run_until_parked();
        assert_eq!(
            confirmed.borrow().as_slice(),
            &[NyaCommandIndex { section: 0, row: 1 }]
        );
        assert_eq!(raw_keydown_count.get(), 0);
    }
}

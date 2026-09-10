use gpui::{App, SharedString, Window};
use gpui_component::{
    WindowExt as _,
    notification::{Notification, NotificationType},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NyaNotificationKind {
    Success,
    Warning,
    Error,
}

/// In-app operation feedback. Stable operation keys replace duplicate toasts.
pub trait NyaNotificationWindowExt {
    fn notify_operation(
        &mut self,
        key: impl Into<SharedString>,
        kind: NyaNotificationKind,
        message: impl Into<SharedString>,
        cx: &mut App,
    );
}

impl NyaNotificationWindowExt for Window {
    fn notify_operation(
        &mut self,
        key: impl Into<SharedString>,
        kind: NyaNotificationKind,
        message: impl Into<SharedString>,
        cx: &mut App,
    ) {
        let notification = Notification::new()
            .id1::<NyaNotificationKind>(key.into())
            .message(message)
            .with_type(match kind {
                NyaNotificationKind::Success => NotificationType::Success,
                NyaNotificationKind::Warning => NotificationType::Warning,
                NyaNotificationKind::Error => NotificationType::Error,
            })
            .autohide(kind == NyaNotificationKind::Success);
        self.push_notification(notification, cx);
    }
}

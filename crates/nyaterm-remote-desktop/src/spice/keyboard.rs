//! Explicit guest shortcuts for combinations reserved by the host desktop.
use super::protocol::SpiceInput;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuestShortcut {
    SecureAttention,
    NextWindow,
    PreviousWindow,
    CloseWindow,
    StartMenu,
    ShowDesktop,
    LockDesktop,
    VirtualTerminal(u8),
}

impl GuestShortcut {
    /// One ordered IPC batch releases stale host modifiers, presses the chord,
    /// and releases in reverse order. No guest key remains held after a click.
    pub fn events(self) -> Vec<SpiceInput> {
        let keys: &[u16] = match self {
            Self::SecureAttention => &[0x1d, 0x38, 0x153],
            Self::NextWindow => &[0x38, 0x0f],
            Self::PreviousWindow => &[0x38, 0x2a, 0x0f],
            Self::CloseWindow => &[0x38, 0x3e],
            Self::StartMenu => &[0x15b],
            Self::ShowDesktop => &[0x15b, 0x20],
            Self::LockDesktop => &[0x15b, 0x26],
            Self::VirtualTerminal(number) => {
                let scan_code = match number {
                    1..=10 => 0x3a + u16::from(number),
                    11 => 0x57,
                    12 => 0x58,
                    _ => return Vec::new(),
                };
                return chord(&[0x1d, 0x38, scan_code]);
            }
        };
        chord(keys)
    }
}

fn chord(keys: &[u16]) -> Vec<SpiceInput> {
    std::iter::once(SpiceInput::ReleaseAll)
        .chain(keys.iter().map(|&scan_code| SpiceInput::Key {
            scan_code,
            pressed: true,
        }))
        .chain(keys.iter().rev().map(|&scan_code| SpiceInput::Key {
            scan_code,
            pressed: false,
        }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::GuestShortcut;
    use crate::spice::protocol::SpiceInput;

    #[test]
    fn guest_shortcuts_balance_every_press_and_release_stale_inputs_first() {
        let shortcuts = [
            GuestShortcut::SecureAttention,
            GuestShortcut::NextWindow,
            GuestShortcut::PreviousWindow,
            GuestShortcut::CloseWindow,
            GuestShortcut::StartMenu,
            GuestShortcut::ShowDesktop,
            GuestShortcut::LockDesktop,
        ];
        for shortcut in shortcuts
            .into_iter()
            .chain((1..=12).map(GuestShortcut::VirtualTerminal))
        {
            let events = shortcut.events();
            assert_eq!(events.first(), Some(&SpiceInput::ReleaseAll));
            let mut held = Vec::new();
            for event in &events[1..] {
                match event {
                    SpiceInput::Key {
                        scan_code,
                        pressed: true,
                    } => held.push(*scan_code),
                    SpiceInput::Key {
                        scan_code,
                        pressed: false,
                    } => assert_eq!(held.pop(), Some(*scan_code)),
                    _ => panic!("unexpected shortcut input"),
                }
            }
            assert!(held.is_empty());
        }
        assert!(GuestShortcut::VirtualTerminal(0).events().is_empty());
        assert!(GuestShortcut::VirtualTerminal(13).events().is_empty());
    }

    #[test]
    fn secure_attention_and_high_function_keys_use_extended_set_one_codes() {
        assert!(
            GuestShortcut::SecureAttention
                .events()
                .contains(&SpiceInput::Key {
                    scan_code: 0x153,
                    pressed: true,
                })
        );
        for (number, code) in [(1, 0x3b), (10, 0x44), (11, 0x57), (12, 0x58)] {
            assert!(
                GuestShortcut::VirtualTerminal(number)
                    .events()
                    .contains(&SpiceInput::Key {
                        scan_code: code,
                        pressed: true,
                    })
            );
        }
    }
}

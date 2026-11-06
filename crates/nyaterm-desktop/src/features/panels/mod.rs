mod about_dialog;
mod connection_import_accounts;
mod connection_import_overlay;
mod helpers;
mod lock_screen_overlay;
mod multi_line_paste_overlay;
mod quick_command_category_overlays;
mod quick_command_details_overlay;
mod quick_command_editor_overlay;
mod quick_command_import_overlay;
mod quick_command_variable_overlay;
mod quick_commands_panel;
mod quick_switch_overlay;
mod recording_panel;
mod session_overlays;
mod sync_groups_overlay;
mod tab_actions_overlay;
mod temporary_ssh_link_overlay;
mod terminal_actions_overlay;
mod update_overlay;

const TAB_PRESET_COLORS: [(&str, u32); 11] = [
    ("Red", 0xef4444),
    ("Orange", 0xf97316),
    ("Amber", 0xf59e0b),
    ("Yellow", 0xeab308),
    ("Green", 0x22c55e),
    ("Emerald", 0x10b981),
    ("Cyan", 0x06b6d4),
    ("Blue", 0x3b82f6),
    ("Indigo", 0x6366f1),
    ("Purple", 0xa855f7),
    ("Pink", 0xec4899),
];

pub(in crate::features::panels) use helpers::{
    QuickCommandCategoryOption, QuickCommandEditorFieldSpec, filtered_quick_commands,
    quick_command_category_options, quick_command_color, quick_command_editor_field,
    quick_command_editor_script_field, quick_command_icon_mark, quick_command_pin_mark,
    quick_command_single_line, terminal_action_prompt_text,
};

mod send_command_helpers;
use send_command_helpers::send_command_control_group;

mod send_command_bar;
mod send_command_state;
pub(in crate::features) use send_command_state::{
    SendCommandFeatureState, SendCommandPresentationState,
};

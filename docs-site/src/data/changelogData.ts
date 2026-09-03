export type ChangelogSection = {
  title: string;
  items: string[];
};

export type ChangelogRelease = {
  version: string;
  sections: ChangelogSection[];
};

const changelogReleasesEn: ChangelogRelease[] = [
  {
    version: '[1.2.8] - 2026-09-03',
    sections: [
      {
        title: 'Added',
        items: [
          "**ssh:** Prune NyaTerm's Bash shell-integration injection commands from remote command history, including shells with `cmdhist` disabled.",
        ],
      },
      {
        title: 'Changed',
        items: [
          "**editor:** Restyle the built-in editor's search panel with theme-aware controls, focus rings, and checkbox accents.",
          '**ssh:** Keep shell-integration injection lines within remote Linux PTY canonical-input limits.',
        ],
      },
    ],
  },
  {
    version: '[1.2.7] - 2026-09-02',
    sections: [
      {
        title: 'Added',
        items: [
          '**editor:** Add Ctrl+mouse-wheel font zoom in the built-in file editor.',
          '**quick-commands:** Add a terminal context-menu action to save the current selection as a quick command.',
          '**terminal:** Add dynamic terminal tab titles.',
          '**sftp:** Add shared helpers for directory listing and file-stat commands.',
          '**ai:** Improve AI stream control event handling and add localized MCP error messages.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**file-drop:** Improve nested drop-zone detection by accepting only the topmost eligible drop target.',
          '**quick-commands:** Preserve the terminal selection while saving it as a quick command.',
          '**terminal:** Scroll to the cursor when typing with a selection and restore focus after switching tabs.',
          '**terminal:** Restore tab titles when waking a hibernated session fails.',
          '**rdp:** Preserve the configured jump host when editing an RDP connection.',
        ],
      },
    ],
  },
  {
    version: '[1.2.6] - 2026-09-01',
    sections: [
      {
        title: 'Added',
        items: [
          '**mcp:** Add the MCP sidecar, active-session reporting, session-open requests, permission modes, and Windows ACL-based discovery.',
          '**security:** Split screen locking into independent startup-lock and idle-lock settings.',
          '**ssh:** Add SSH config import with ProxyJump and multi-hop connection support.',
          '**sftp:** Add configurable pipeline depth and symlink-target management.',
          '**terminal:** Add configurable right-click actions and saving selected text as a quick command.',
          '**asset-monitoring:** Improve incremental asset patches and session validation.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**terminal:** Streamline context menus and improve refresh and keyword-highlighting effects.',
          '**dependencies:** Update Tauri and Rust dependencies and add Windows security features for MCP discovery.',
          '**network:** Refresh tunnel row and runtime badge layout and styling.',
          '**i18n:** Add translations for MCP permissions, SFTP pipeline depth, terminal actions, symlinks, and lock settings.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**clipboard:** Support paste from Windows clipboard history in the terminal.',
          '**serial-send:** Preserve panel input across hide and show.',
          '**editor:** Reuse the terminal selection color in the built-in file editor.',
          '**ai:** Prevent IME composition Enter from submitting an AI prompt.',
        ],
      },
      {
        title: 'Performance',
        items: ['**terminal:** Add output acknowledgement coordination, snapshot restoration, and more reliable refresh behavior.'],
      },
      {
        title: 'Documentation',
        items: ['**development:** Document all four locale files used by the project.'],
      },
    ],
  },
  {
    version: '[1.2.5] - 2026-08-24',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal:** Add hibernation and output control, image rendering, foreground-frame fallback, and improved background draining.',
          '**credential-autofill:** Detect credential prompts and offer saved credential completion.',
          '**file-explorer:** Automatically synchronize with the terminal working directory.',
          '**zmodem:** Improve upload conflict handling and surface remote upload refusals.',
          '**send-command-panel:** Add an all-windows target and an option to clear serial input after sending.',
          '**settings:** Add drag-and-drop ordering for terminal font stacks.',
          '**windows-7:** Add fixed WebView2 runtime support and dedicated build workflows.',
          '**ai-settings:** Add selectable API formats and improve model-provider defaults.',
          '**remote-desktop:** Add proxy and jump-host selection for RDP and VNC and improve RDP error reporting.',
          '**custom-icons:** Add custom connection icon management.',
          '**ssh:** Add runtime modes for external invocation and protocol links.',
          '**activity-bar:** Add visibility management and reset actions.',
          '**floating-panels:** Add floating mode and persisted panel state.',
          '**quick-commands:** Add command-variable parsing and interactive value resolution.',
          '**editor:** Add configurable internal and external file-open modes.',
          '**session:** Add external local-session invocation with working-directory validation.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**terminal:** Improve transparent backgrounds and event-listener lifecycle management.',
          '**app:** Improve child-window lifecycle handling on macOS and consolidate overlay dialogs.',
          '**ai:** Improve session-history request tracking and error handling.',
          '**i18n:** Add and update localized text for the new settings and workflows.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**terminal:** Reset the running-command state when OSC 133 reports command boundaries.',
          '**webview:** Add guarded hard-reload shortcut handling.',
          '**ai:** Prevent IME composition Enter from triggering prompt submission.',
        ],
      },
      {
        title: 'Documentation',
        items: ['**ssh:** Document local invocation, connection link formats, and RDP/VNC proxy and jump-host support.'],
      },
    ],
  },
  {
    version: '[1.2.4] - 2026-08-17',
    sections: [
      {
        title: 'Added',
        items: [
          '**file-editor:** Add text editing with unsaved-change protection and safe close handling.',
          '**sftp:** Classify text files for editor opening while preserving underlying errors.',
          '**ssh:** Add configurable SSH Agent forwarding.',
          '**file-explorer:** Add a move dialog with validation and tests.',
          '**terminal-session:** Improve working-directory handling for local and WSL commands.',
          '**child-window:** Add a drag-only macOS header region and recoverable command queues.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**saved-connections:** Prevent duplicate folder submissions.',
          '**context-menu:** Prevent right-click from activating menu items.',
          '**local-terminal:** Improve shell-path input behavior and localization.',
          '**macos:** Align child-window traffic-light controls more consistently.',
        ],
      },
      {
        title: 'Performance',
        items: [
          '**terminal:** Add alternate-screen tracking, output scheduling, and the DEC 2026 frame gate.',
          '**window:** Reduce child-window startup work and loading time.',
        ],
      },
      {
        title: 'Documentation',
        items: ['**ssh:** Document external invocation and supported protocol links.'],
      },
    ],
  },
  {
    version: '[1.2.3] - 2026-08-13',
    sections: [
      {
        title: 'Added',
        items: [
          '**vnc:** Add VNC sessions with saved-connection support, workspace integration, authentication, clipboard handling, input forwarding, and framebuffer rendering.',
          '**remote-desktop:** Add shared frame, viewport, renderer, and surface utilities for VNC and RDP panes.',
          '**terminal:** Add session recording and transcript management.',
          '**asset-management:** Add connection time tracking, display formatting, asset sorting, and persisted sort state.',
          '**cloud-sync:** Add a cloud snapshot decoding helper and integrate it into the main sync flow.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**rdp:** Improve RDP behavior through shared remote desktop rendering and viewport handling.',
          '**settings:** Normalize settings tab handling and update terminal context menu behavior.',
          '**process-manager:** Optimize process display settings and remove unused process-management code.',
          '**ui:** Remove unnecessary transparent background styling from the wallpaper surface.',
          '**tuning:** Change release panic behavior from unwind to abort.',
          '**i18n:** Add and update localization strings for VNC, recording, and connection time across supported languages.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**cloud-sync:** Validate source hashes while decoding snapshots to improve sync data integrity.',
          '**temporary-session:** Complete temporary session recreation flows and fix context-menu actions for temporary SSH links.',
        ],
      },
      {
        title: 'Documentation',
        items: [
          '**readme:** Update README content for RDP and VNC support.',
          '**docs-site:** Update session type documentation with RDP and VNC support.',
        ],
      },
    ],
  },
  {
    version: '[1.2.2] - 2026-08-12',
    sections: [
      {
        title: 'Added',
        items: [
          '**build:** Add Windows MSI packaging support to the release workflow.',
          '**quick-commands:** Add drag-and-drop sorting for quick command categories.',
          '**rdp:** Enhance RDP resizing and dynamic display behavior.',
          '**header:** Add predefined macOS edit menu items.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**terminal-gutter:** Improve gutter layout constants for better readability.',
          '**i18n:** Add quick command labels for category creation and custom sorting.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**rdp:** Improve physical key capture, lock-key routing, right Shift handling, and UPN username preservation during authentication.',
          '**terminal:** Reset timestamps to terminal startup time when Clear All is used.',
          '**release:** Prevent release crashes when remote metadata is corrupt.',
        ],
      },
      {
        title: 'Documentation',
        items: ['**readme:** Update Discord and WeChat group links.'],
      },
    ],
  },
  {
    version: '[1.2.1] - 2026-08-11',
    sections: [
      {
        title: 'Added',
        items: [
          '**rdp:** Add RDP sessions with workspace integration, certificate verification, text clipboard support, fit-window/fixed display modes, bounded reconnects, and improved keyboard/IME handling.',
          '**ssh:** Add SSH Agent authentication and forwarding, plus SSH profiles and terminal type selection for server and network-device workflows.',
          '**cloud-sync:** Add current remote snapshot metadata, automatic pull of remote-only changes, conflict recovery actions, and snapshot cleanup for legacy objects.',
          '**terminal:** Add a high-volume output drain, gutter refresh support, timestamp restoration, command suggestion shortcuts, new tab/view commands, and quick switcher scrolling.',
          '**recording:** Add per-connection recording settings and expand capture, formatting, search, and test support.',
          '**quick-commands:** Add category movement, sorting, export support, validation improvements, and import merge behavior that preserves categories.',
          '**saved-connections:** Add expand/collapse-all folder actions with persisted expansion state.',
          '**file-explorer:** Add selected and total size statistics in the file explorer footer.',
          '**macos-menu:** Add native macOS menu handling and window-management commands.',
          '**serial-send:** Enhance serial send state management.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**settings:** Share draft-state handling across settings pages so manual sync actions and unsaved changes are coordinated more consistently.',
          '**layout:** Improve resize handle interaction styling and ActivityBar background consistency.',
          '**claude:** Refactor Claude Code invocation handling for agent execution flows.',
          '**i18n:** Add and refresh localization for RDP, SSH Agent/profile settings, cloud sync recovery, recording, terminal suggestions, file selection totals, macOS menus, and quick-command export.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**rdp:** Improve cursor handling, viewport resizing, and keyboard input behavior.',
          '**ssh:** Increase command injection timeout to improve reliability on slower shells.',
          '**app-context:** Restrict screen locking to the primary main window.',
          '**about:** Improve support information handling and display.',
        ],
      },
      {
        title: 'Performance',
        items: ['**terminal:** Improve responsiveness during large output bursts with more efficient output draining.'],
      },
      {
        title: 'Documentation',
        items: ['**readme:** Add Discord and WeChat group badges for community engagement.'],
      },
    ],
  },
  {
    version: '[1.2.0] - 2026-08-04',
    sections: [
      {
        title: 'Added',
        items: [
          '**gpu-npu-monitor:** Add remote GPU and NPU overviews to the application header/status area, with shared overview hooks and compact hardware cards for active SSH sessions.',
          '**ui:** Add a Notes panel toggle to settings and include Notes in the default workspace configuration.',
          '**file-explorer:** Support selecting multiple directories for uploads.',
          '**terminal:** Show the saved-connection group path in TabBar tooltips.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**monitoring:** Integrate remote GPU/NPU overview data with the existing GPU and Ascend NPU monitor panels.',
          '**i18n:** Add localization strings for GPU/NPU overviews, Notes, and saved-connection group labels across supported languages.',
        ],
      },
    ],
  },
  {
    version: '[1.1.19] - 2026-08-04',
    sections: [
      {
        title: 'Added',
        items: [
          '**notes:** Add a Notes panel and note editor with autosave, toolbar/status UI, tree navigation, context menus, persistent storage, migrations, and sync/backup snapshot support.',
          '**asset-monitoring:** Add an asset monitoring workspace with grouped connection views, breadcrumb navigation, table/card layouts, formatters, and resource/GPU monitor integration.',
          '**sftp:** Enhance directory downloads and transfer handling, including improved pipeline behavior and SCP original-property handling.',
          '**zmodem:** Add local path tracking for Zmodem transfers and reveal-in-file-manager support.',
          '**quick-commands:** Add import preservation options for merging quick commands.',
          '**ai:** Add native tool-call mode with error handling for agent workflows.',
          '**import:** Improve WindTerm session import, including master-password prompts and merge handling.',
          '**external-open:** Enhance SSH URL parsing and external connection handling.',
          '**capabilities:** Allow hidden files in the temporary directory capability.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**monitoring:** Improve system information collection in the remote stats script.',
          '**ui:** Simplify the Asset toolbar by removing unused title/count display.',
          '**i18n:** Add and update localization for notes, asset monitoring, WindTerm master-password prompts, CPU sampling, and external inline-password guidance.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**monitoring:** Handle `null` values in CPU usage and percentage formatting.',
          '**terminal:** Refocus the search input when Find is triggered while the search bar is already open.',
        ],
      },
      {
        title: 'Performance',
        items: ['**notes:** Refactor the Notes panel and tree components for better performance and usability.'],
      },
      {
        title: 'Documentation',
        items: ['**ssh:** Clarify SSH connection guidance around inline password usage.'],
      },
    ],
  },
  {
    version: '[1.1.18] - 2026-07-29',
    sections: [
      {
        title: 'Added',
        items: [
          '**external-open:** Add external connection handling, URL parsing, saved-connection matching, and confirmation dialogs for opening connection links from outside NyaTerm.',
          '**terminal:** Add `TerminalFitScheduler` with tests, improve search lifecycle hooks, add external file-drop handling, and refresh terminal layout/output more reliably after reconnect and visibility changes.',
          '**session:** Add session attachment support and hibernation logging.',
          '**sftp:** Add remote file copy operations with controller support, and preserve permissions during remote file write operations.',
          '**file-explorer:** Add an editable path button, search entry, active session targeting, and improved file-window target handling.',
          '**header:** Add header status visibility controls, confirmation dialogs, and refreshed header actions/icons.',
          '**security-auth:** Add credential, private key, and OTP management dialogs, plus improved key-management UI and backend handling.',
          '**network:** Add Proxy and Tunnel management pages.',
          '**quick-commands:** Add command preview, copy support, and category selection state management.',
          '**ai-settings:** Add AI model refresh and improve model discovery.',
          '**updater:** Add automatic updates for Windows portable builds.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**sftp:** Modularize the SFTP backend and improve upload cancellation handling.',
          '**importer:** Unify secret encryption logic across importers.',
          '**updater:** Unify portable updates on Cloudflare R2.',
          '**docker:** Update Docker command paths.',
          '**settings:** Improve close handling and unsaved-change confirmation.',
          '**i18n:** Update credential, SSH key, proxy/tunnel, terminal display, header status, and external-connection localization.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**terminal:** Improve terminal snapshot serialization and reconnect handling, refresh timestamps on Enter and suggestion execution, and improve reconnect/session handling for synchronized input.',
          '**sync:** Use session input peer IDs for sync peer selection.',
          '**window:** Avoid child-window loading flashes and improve child-window logging/conflict warnings.',
          '**sftp:** Improve directory removal logic with raw path matching.',
          '**file-explorer:** Improve resize handling in the path bar.',
          '**ssh:** Increase command injection timeout from 5 to 30 seconds.',
          '**redaction:** Add marker-value redaction for sensitive data.',
          '**ui:** Improve responsive class names in SSH and new-session forms.',
        ],
      },
      {
        title: 'CI',
        items: [
          '**updater:** Add signed portable target verification for `latest.json`.',
          '**gitee:** Improve Python dependency installation in the release workflow.',
        ],
      },
      {
        title: 'Documentation',
        items: ['**readme:** Update badge links and formatting in the Chinese README.'],
      },
    ],
  },
  {
    version: '[1.1.17] - 2026-07-22',
    sections: [
      {
        title: 'Fixed',
        items: [
          '**csp:** Allow `blob:` image URLs in the content security policy so generated or previewed image assets can render correctly.',
          '**terminal:** Improve hibernation logic in the XTerminal component.',
          '**zmodem:** Improve conflict resolution handling during Zmodem uploads.',
        ],
      },
      {
        title: 'Documentation',
        items: [
          '**readme:** Add macOS installation instructions to the README files.',
          '**i18n:** Clarify Zmodem conflict resolution messages.',
        ],
      },
    ],
  },
  {
    version: '[1.1.16] - 2026-07-21',
    sections: [
      {
        title: 'Added',
        items: [
          '**temporary-link:** Consolidate temporary connection handling and extend protocol support for one-off sessions.',
          '**file-explorer:** Add file copy operations, backend file-operation support, raw path-token handling, directory navigation caching, and file preview flows.',
          '**quick-commands:** Add a script editor for quick commands and support copying commands.',
          '**terminal:** Add customizable timestamp formats, SSH keep-alive mode settings, disconnected output flushing, improved command-suggestion keyboard navigation, and disconnected tab indicators.',
          '**settings:** Add app-language saving so language changes persist through the backend.',
          '**icons:** Add Kubernetes, macOS, and Android icon assets, and improve remote system icon auto-detection after session start.',
          '**zmodem:** Add upload timestamp preservation and safer conflict prompts for unverified destinations.',
          '**saved-connections:** Add keyboard navigation, connection element registration, and move-to-group actions.',
          '**ai:** Add Codex and Claude Code integration, run-mode selection, model filtering, custom provider settings, command target context, and dynamic tool-call tracking.',
          '**clipboard:** Improve clipboard writing on Windows and fallback behavior.',
          '**cloud-sync:** Add connection success handling and improve portable snapshot handling for sync and backup modes.',
          '**theme:** Add a theme designer dialog and enhanced theme management.',
          '**i18n:** Add Traditional Chinese localization and language handling.',
          '**session-targets:** Enhance Send Command target structure and compatibility.',
          '**tunnel:** Add tunnel runtime state management and connection tracking.',
          '**tray:** Add macOS tray icon support.',
          '**monitoring:** Add an Ascend NPU monitoring panel and settings.',
          '**header:** Add remote stats support and header status mode.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**header:** Streamline session icon handling.',
          '**ssh:** Rename and update suppressed output handling helpers.',
          '**terminal:** Update local PTY environment configuration for cross-platform compatibility.',
          '**sftp:** Normalize remote directory paths and improve path joining.',
          '**settings:** Rename macOS IME compatibility to a general IME compatibility setting.',
          '**ai:** Remove redundant Markdown rendering from the agent step view.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**clipboard:** Keep the arboard clipboard alive and enable Wayland support.',
          '**context:** Refactor startup restore logic for tab sessions.',
          '**panel:** Improve pointer-event handling in saved connection items and group nodes.',
          '**cloud-sync:** Initialize OpenDAL transport for all providers and upgrade OpenDAL to 0.58.',
          '**build:** Use `ring` instead of `aws-lc-rs` for the russh crypto backend.',
          '**terminal:** Fix GBK encoding support and duplicate input with Fcitx IME on Linux.',
          '**icons:** Improve remote system distribution matching.',
        ],
      },
      {
        title: 'Performance',
        items: [
          '**terminal:** Improve terminal output handling, performance mode behavior, and WebGL resource management.',
        ],
      },
      {
        title: 'Documentation',
        items: [
          '**i18n:** Add SSH keep-alive descriptions and update localization for new workflows.',
          '**images:** Update product images for dark and light themes.',
        ],
      },
    ],
  },
  {
    version: '[1.1.15] - 2026-07-14',
    sections: [
      {
        title: 'Fixed',
        items: ['**app:** Remove macOS-specific tab close handling and streamline effect dependencies.'],
      },
      {
        title: 'CI',
        items: ['**gitee:** Improve the release workflow trigger and tag resolution.'],
      },
    ],
  },
  {
    version: '[1.1.14] - 2026-07-14',
    sections: [
      {
        title: 'Added',
        items: [
          '**telnet:** Add auto-login and startup command support.',
          '**tabs:** Add tab locking to prevent accidental closure.',
          '**terminal:** Add a remote color OSC guard for serial sessions, terminal zoom settings, enhanced theming and layout adjustments, and standardized input behavior.',
          '**remote-stats:** Integrate remote system statistics and connection icon auto-detection.',
          '**ai:** Add reasoning-effort selection to model controls.',
          '**connections:** Add multi-select delete support and restore the last opened connection and expanded group state.',
          '**webview:** Prevent reserved shortcuts from being intercepted by webview content.',
          '**fonts:** Add JetBrainsMono Nerd Font Mono and update font defaults.',
          '**xterm:** Isolate the xterm WebGL texture atlas for NyaTerm.',
          '**build:** Add unlock rescue scripts and a post-install xterm WebGL patch.',
          '**docker:** Add Docker sudo password handling.',
          '**security-auth:** Add saved-password authentication support, credential unlock actions, credential reordering, Tab-key credential selection, and active-session-aware OTP management.',
          '**appearance:** Add window transparency controls and improved font handling.',
          '**i18n:** Add Korean localization.',
          '**sftp:** Add SFTP settings and a remote file browser.',
          '**header:** Add an unsplit action.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**terminal:** Improve clipboard behavior in the terminal context menu, prevent stale macOS trackpad selection, and support Ctrl printable terminal keys.',
          '**stats:** Ensure disk data files are created and populated correctly.',
          '**panel:** Fix progress calculation for multiple units.',
          '**network:** Limit connection options to SSH connections.',
          '**ssh:** Ensure the post-login timer only arms during the normal phase.',
        ],
      },
      {
        title: 'Performance',
        items: [
          '**terminal:** Improve output handling and flow control.',
          '**zmodem:** Add upload and download drain mechanisms.',
        ],
      },
      {
        title: 'Documentation',
        items: [
          '**readme:** Update contributor image links.',
          '**installation:** Add direct Windows portable download links.',
          '**images:** Add monitoring and import images for the docs site.',
          '**changelog:** Update changelog entries through `1.1.13`.',
        ],
      },
    ],
  },
  {
    version: '[1.1.13] - 2026-07-06',
    sections: [
      {
        title: 'Added',
        items: [
          '**gpu:** Add a GPU monitoring panel for SSH hosts, showing driver/CUDA versions, per-GPU utilization, memory, temperature, power, fan, and a searchable per-process GPU usage list, with a `Show GPU Monitor` toggle and configurable poll interval.',
          '**docker:** Add a Docker management panel for SSH hosts covering containers, images, volumes, networks, and Compose projects, with container details, logs, exec/enter, lifecycle actions, and confirmed destructive operations such as remove, kill, compose down, and `system prune`.',
          '**process:** Add a process management panel for SSH hosts with a live, sortable, searchable process list, per-process detail, `renice`, and signal sending (TERM/HUP/STOP/CONT and confirmed KILL).',
          '**app:** Track live session state so monitor panels bind only to a genuinely active SSH session, and gate activity-panel visibility so toggled-off panels are not auto-opened.',
          '**ssh:** Add per-connection SSH algorithm preferences with Compatible / Secure / Custom modes, reorderable key-exchange, cipher, MAC, and host-key lists, and Modern/Legacy/Insecure risk labels.',
          '**connections:** Add a temporary SSH link dialog that opens a one-off session from a pasted `ssh://` URL or `ssh` command string without saving a connection.',
          '**terminal:** Add session input synchronization via named sync groups, mirroring keystrokes to grouped peers, plus a Send Command target selector for current session, all sessions, or a specific group.',
          '**file-transfer:** Surface Zmodem (rz/sz) transfers in the transfer list with progress and Zmodem-appropriate controls.',
          '**terminal:** Add Unicode grapheme support for correct rendering and cursor width of emoji, combining marks, and ZWJ sequences.',
          '**terminal:** Add a Clear Input action (Ctrl/Cmd+L) available from the context menu and keyboard shortcut.',
          '**terminal:** Add a Close All Sessions confirmation dialog before closing all tabs.',
          '**ai:** Add a background execution setting for the AI Assistant.',
          '**app-lock:** Add window close confirmation and control to the lock screen.',
          '**window:** Enhance main window state management and positioning.',
          '**importer:** Extract user and host from WindTerm session imports.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**terminal:** Expand built-in keyword highlight presets with more error and success phrases.',
          '**ai:** Improve model name handling for the Deepseek provider.',
          '**serial:** Refine the serial session form layout for better responsiveness.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**russh:** Improve name-list parsing to handle trailing commas and reject invalid entries.',
          '**ssh:** Improve X11 channel handling in the SSH handler.',
          '**ssh:** Suppress the flashing console window when running local system shell commands on Windows.',
          '**hooks:** Add reload handling for forced credential loading.',
        ],
      },
      {
        title: 'Performance',
        items: [
          '**sftp:** Add channel-concurrency limiting and automatic retry with backoff for transient SFTP channel-open failures.',
          '**watcher:** Use content fingerprinting so auto-upload triggers only on real content changes, not editor metadata-only saves.',
          '**hooks:** Add a forced-reload option to credential loading.',
        ],
      },
    ],
  },
  {
    version: '[1.1.12] - 2026-06-30',
    sections: [
      {
        title: 'Added',
        items: [
          '**connections:** Enhance jump host configuration with clearer chain handling and validation for ProxyJump-style setups.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**connections:** Prevent cycles in jump host relationships so saved connections cannot reference each other recursively.',
        ],
      },
      {
        title: 'Documentation',
        items: ['**i18n:** Add localization strings for the updated jump host workflow.'],
      },
    ],
  },
  {
    version: '[1.1.11] - 2026-06-30',
    sections: [
      {
        title: 'Added',
        items: [
          '**header:** Add a Command Palette entry for quickly finding app actions and session workflows.',
          '**ai:** Introduce command-execution and final-answer tools for Agent interactions while preserving approval gates.',
          '**app-lock:** Implement app lock state management with idle locking support.',
          '**terminal-layout:** Restore terminal window layout state and add related settings for persistent workspaces.',
          '**appearance:** Add terminal font weight options for normal and bold text rendering.',
          '**app:** Add minimize-to-tray and hide-main-window behavior for background workflows.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**window-management:** Improve child-window state persistence and modal focus handling.',
          '**themes:** Refresh terminal and app theme colors, including high-contrast variants.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**saved-connections:** Improve pointer-event handling for connection interactions and drag behavior.',
          '**terminal:** Improve terminal zoom handling across nested workspace roots.',
        ],
      },
    ],
  },
  {
    version: '[1.1.10] - 2026-06-25',
    sections: [
      {
        title: 'Added',
        items: [
          '**ssh-auth:** Add a dedicated SSH authentication request dialog for interactive login flows.',
          '**proxy:** Add ProxyCommand support to proxy configuration, including OpenSSH-style placeholders.',
          '**transfer-dialog:** Add duplicate target handling options for uploads and downloads.',
          '**terminal:** Improve terminal search with persistent query state, result navigation, and clearer feedback.',
          '**cloud-sync:** Add more detailed cloud sync status updates and cleanup timeout handling.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**ai:** Streamline AI model client service-target configuration.',
          '**cloud-sync:** Adjust cloud-sync history and log handling for clearer diagnostics.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**terminal:** Improve search focus handling and selection clearing.',
          '**settings:** Fix saved connection sort order on the new session page.',
        ],
      },
      {
        title: 'Performance',
        items: ['**serial:** Streamline serial session management and timeout handling.'],
      },
    ],
  },
  {
    version: '[1.1.9] - 2026-06-21',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal:** Add startup commands for duplicated and multiplexed sessions.',
          '**terminal:** Add image path pasting behavior in terminal settings.',
          '**terminal:** Add workspace padding settings for terminal layout spacing.',
          '**interaction:** Add a macOS IME compatibility setting.',
          '**telnet:** Add local line editing for Telnet sessions.',
          '**terminal:** Enhance Windows Terminal support and shell selection.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**terminal:** Suppress command suggestions in interactive programs where inline suggestions are more likely to interfere.',
          '**sync-backup:** Streamline sync backup history and cloud sync manager behavior.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**tabbar:** Prevent pointer event propagation issues in tab interactions.',
          '**terminal:** Improve reconnect content management and connection error handling.',
        ],
      },
    ],
  },
  {
    version: '[1.1.8] - 2026-06-21',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal:** Add tab splitting with drag-and-drop docking support.',
          '**ssh:** Add X11 forwarding support for SSH connections.',
          '**russh-sftp:** Introduce a new SFTP subsystem with client and server support.',
          '**file-explorer:** Add remote file editor flows and dedicated remote-file handling dialogs.',
          '**sftp:** Add OpenSSH-compatible symlink support.',
          '**file-transfer:** Show transfer speed in the transfer UI.',
          '**cloud-sync:** Enhance cloud sync and GitHub Gist startup/error handling.',
          '**saved-connections:** Add copy shortcuts and improve drag-and-drop behavior for selected connections.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**scp:** Add compatibility checks for GNU `-printf` and `-c` support in remote commands.',
          '**file-explorer:** Improve directory loading, download handling, and error feedback.',
        ],
      },
      {
        title: 'Performance',
        items: [
          '**russh-sftp:** Improve SFTP upload throughput and transfer tracking.',
          '**sftp:** Use asynchronous downloads when file sizes are known.',
        ],
      },
      {
        title: 'Documentation',
        items: [
          '**ssh:** Document X11 forwarding in the SSH connection guide.',
          '**cloud-sync:** Update cloud sync terminology and GitHub Gist authorization guidance.',
        ],
      },
    ],
  },
  {
    version: '[1.1.7] - 2026-06-15',
    sections: [
      {
        title: 'Added',
        items: [
          '**panel:** Add multi-open panel behavior for opening multiple tools without losing context.',
          '**terminal:** Add backend output pause/resume controls.',
          '**session:** Add a session quick switcher dialog for faster navigation between active sessions and saved connections.',
          '**icons:** Add server icons and improve saved connection icon resolution.',
          '**network:** Add group management for proxies and tunnels.',
          '**sync:** Add v3 snapshot decoding and payload hash calculation.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**file-explorer:** Define a shared `FileProperties` interface and clean up the Properties dialog.',
          '**terminal:** Improve tab tooltips, tab management, and scrolling behavior.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**command-history:** Correct suggestion application behavior.',
          '**app:** Improve Windows folder opening when permission errors occur.',
          '**file-explorer:** Fix opening the target directory after remote-file downloads.',
        ],
      },
    ],
  },
  {
    version: '[1.1.6] - 2026-06-12',
    sections: [
      {
        title: 'Added',
        items: [
          '**session:** Add a searchable session quick switcher to jump between saved connections and local sessions from the keyboard or mouse, including a shortcut entry for creating a new SSH session.',
          '**recording:** Add an auto-start recording option that begins recording automatically when a session opens.',
          '**terminal:** Serialize terminal text so reconnecting sessions restore their previous on-screen output.',
          '**terminal:** Allow disconnected panes to be closed.',
          '**terminal:** Add a command to delete individual command history entries.',
          '**file-explorer:** Enhance file attribute management in the Properties dialog.',
          '**clipboard:** Implement asynchronous clipboard reading with a timeout.',
          '**action-links:** Support RAR archives in the archive action-link matcher.',
          '**ai:** Enhance model management with manual model addition and credential grouping.',
          '**transfer:** Implement background transfer concurrency adjustment.',
          '**stats:** Enhance `SYSINFO_SCRIPT` for improved system information gathering.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**quick-commands:** Introduce a sorting mode and update view mode defaults.',
          '**sftp:** Enhance directory and symlink handling in the SFTP backend.',
          '**password-management:** Enhance unlock logic and footer visibility in the password panel.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**connection:** Improve the password management editing flow.',
          '**macos:** Support drag reordering of connections on macOS.',
          '**file-explorer:** Reset horizontal scroll position and improve file item interaction.',
          '**terminal:** Streamline right-click paste in the terminal context menu.',
          '**ssh:** Reorganize imports and adjust preferred algorithms.',
        ],
      },
      {
        title: 'Performance',
        items: ['Compress portable snapshots before encryption.'],
      },
      {
        title: 'Documentation',
        items: ['**README:** Add Arch Linux installation instructions.'],
      },
    ],
  },
  {
    version: '[1.1.5] - 2026-06-09',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal:** Add multiplexed SSH session support so a single connection can power multiple terminals.',
          '**ai:** Introduce a dedicated AI Assistant panel and related components.',
          '**ai:** Add command risk levels for execution control.',
          '**ai:** Add request User-Agent configuration with sensible default handling.',
          '**cloud-sync:** Add a Gitee snippet cloud sync provider.',
          '**terminal:** Enhance multi-line paste handling with a dedicated dialog and input state management.',
          '**terminal:** Implement local backspace handling in terminal input.',
          '**terminal:** Add support for timestamp milliseconds in the terminal display.',
          '**quick-commands:** Add a compact view mode and a view mode toggle, plus a confirmation dialog for command deletion.',
          '**file-explorer:** Implement a favorites feature for directories.',
          '**key-management:** Add certificate file handling in the key editor.',
          '**search:** Improve search engine management with dynamic key generation.',
          '**local-terminal:** Add shell arguments support and file selection for the shell path.',
          '**pty:** Add a local startup script for shell integration, with output suppression during startup.',
          '**themes:** Add the Nya High Contrast theme and refresh the color palette.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**app:** Update the window title to reflect the active tab name.',
          '**window:** Implement owner window label handling for child windows and expand window capability patterns.',
          '**ssh:** Enhance prompt-injection handling and OSC processing.',
          '**ssh:** Enhance password prompt handling in keyboard-interactive authentication.',
          '**ai:** Make AI output follow the app language via locale-based prompt selection.',
          '**credential-management:** Improve regex validation and prompt handling.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**deps:** Update xterm dependencies to beta versions.',
          '**macos:** Normalize the packaged macOS PTY environment.',
          '**terminal:** Initialize disconnect and reconnect states in XTerminal.',
        ],
      },
    ],
  },
  {
    version: '[1.1.4] - 2026-06-03',
    sections: [
      {
        title: 'Added',
        items: [
          '**ssh:** Implement post-login command support in the SSH form and new session page.',
          '**ssh:** Add a required-field indicator for the password field in the SSH form.',
          '**saved-connections:** Enhance search with expanded group management.',
          '**serial:** Enhance serial session handling with improved error logging and Zmodem detection.',
          '**panel:** Enhance the send-command panel with hex data handling and refactored state management.',
          '**recording:** Add a timestamp inclusion option for recordings.',
          '**terminal:** Enhance the terminal gutter with dynamic cell dimensions and layout adjustments, and improve the overall input experience.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**zmodem:** Improve Zmodem event handling and detection logic.',
          '**terminal:** Unify cursor position handling for command and credential suggestions.',
          '**terminal:** Improve multi-line paste dialog focus handling.',
        ],
      },
      {
        title: 'Fixed',
        items: ['**header:** Respect the minimize-to-tray setting when closing the window.'],
      },
    ],
  },
  {
    version: '[1.1.3] - 2026-06-02',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal:** Implement a multi-line paste dialog and enhance paste handling.',
          '**terminal:** Enhance input handling with logical line snapshots and selection range tracking.',
          '**terminal:** Implement credential prompt detection and input handling.',
          '**panel:** Enhance the send-command panel with shell command functionality.',
          '**recording:** Implement session recording, including start/stop, transcript saving, and memory limit settings.',
          '**file-explorer:** Add session-scoped directory history in the path bar.',
          '**import:** Support importing sessions from NyaTerm JSON format in the import dialog.',
          '**security:** Enhance master password management with improved validation and UI updates.',
          '**quick-commands:** Add dialogs for deleting and renaming quick command categories.',
          '**docs-site:** Add offline local search.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**cloud-sync:** Implement operation timeouts and enhance error handling for storage operations.',
          '**cloud-sync:** Add WebDAV Digest authentication support.',
          '**cloud-sync:** Implement an automatic retry mechanism for cloud sync operations.',
          '**backup:** Update the backup file extension from `.dgfy` to `.nya`.',
          '**header:** Rename the menu label from `New SSH Connection` to `New Session`.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**settings:** Ensure the settings window closes after saving by managing saving state.',
          '**app:** Improve `safeRecordingName` normalization to allow a wider range of valid characters.',
          '**docs-site:** Improve navbar responsiveness and the floating search box.',
        ],
      },
    ],
  },
  {
    version: '[1.1.2] - 2026-05-30',
    sections: [
      {
        title: 'Added',
        items: [
          '**window-state:** Implement main window state management to persist window size and position.',
          '**quick-commands:** Add support for importing Xshell quick buttons.',
        ],
      },
      {
        title: 'Changed',
        items: ['**app:** Centralize child window size and position handling.'],
      },
      {
        title: 'Documentation',
        items: ['Add a contributors section and star history chart to the README.'],
      },
    ],
  },
  {
    version: '[1.1.1] - 2026-05-29',
    sections: [
      {
        title: 'Fixed',
        items: ['Remove unused child window preload logic and background color handling.'],
      },
    ],
  },
  {
    version: '[1.1.0] - 2026-05-29',
    sections: [
      {
        title: 'Added',
        items: [
          '**serial:** Implement a baud rate picker for selecting and validating serial baud rates.',
          '**file-transfer:** Add download functionality and enhance transfer management with enqueued downloads.',
          '**file-transfer:** Add a `queued` status and improve transfer UI interactions.',
          '**file-explorer:** Auto-sync the current working directory based on the active connection.',
          '**transfer:** Implement concurrency clamping and rename download/upload thread settings to concurrent tasks.',
          '**errors:** Add new authentication failure messages and enhance validation in the new session page.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**file-explorer:** Add a refreshed FileExplorer component and dialogs for file operations, including path bar and toolbar.',
          '**ui:** Standardize dialog footers with `ActionButton` and `ActionFooter`, and move the Toaster into the main layout.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**modal:** Prevent the modal overlay from remaining after a child window closes.',
          '**child-windows:** Reduce startup flicker with window preloading.',
          '**build-release:** Correct the package name for Ubuntu ARM installation.',
        ],
      },
      {
        title: 'Performance',
        items: ['**sftp:** Enhance the SFTP backend with configurable client settings and performance logging.'],
      },
    ],
  },
  {
    version: '[1.0.9] - 2026-05-27',
    sections: [
      {
        title: 'Added',
        items: [
          '**ssh:** Introduce a `none` authentication mode in the SSH form for enhanced connection options.',
          '**runtime:** Implement portable mode support and add a Windows portable zip release.',
          '**quick-commands:** Add an import dialog for quick commands supporting WindTerm and NyaTerm formats.',
          '**terminal:** Add disconnect session functionality.',
          '**file-explorer:** Implement inline renaming for file entries.',
          '**file-transfer:** Enhance progress tracking and add a clear-all action.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**file-transfer:** Update transfer row status icons and simplify status handling.',
          '**dialogs:** Improve responsive width handling and Markdown rendering in the update dialog.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**session-input:** Fix a quick command issue caused by newline conversion.',
          '**telnet:** Integrate the recording manager for session input/output handling.',
          '**shortcuts:** Validate and support custom tab switching keybindings.',
          '**saved-connections:** Update the empty state to check both saved connections and groups.',
          '**workflows:** Update the Ubuntu version from 24.04 to 22.04 in the release workflow.',
        ],
      },
    ],
  },
  {
    version: '[1.0.8] - 2026-05-24',
    sections: [
      {
        title: 'Added',
        items: [
          '**quick-commands:** Implement sorting and usage tracking for quick commands.',
          '**readme:** Add a GitHub downloads badge.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**settings:** Update app settings management and introduce UI settings saving.',
          '**storage:** Restructure the storage module and migrate to typed settings documents.',
          '**security:** Update master password handling in the security tab.',
          '**scrollbar:** Hide scrollbar buttons and improve scrollbar appearance.',
        ],
      },
      {
        title: 'Fixed',
        items: ['**errors:** Improve error handling in the new session and quick command pages.'],
      },
    ],
  },
  {
    version: '[1.0.7] - 2026-05-21',
    sections: [
      {
        title: 'Changed',
        items: [
          '**appearance:** Improve font selection UI and show a `Loading system fonts...` state while system fonts are being discovered.',
          '**otp:** Improve OTP input slot layout and OTP code panel responsiveness on narrower screens.',
          '**profiles:** Add multiple Cargo build profiles for debugging and release workflows.',
        ],
      },
      {
        title: 'Fixed',
        items: ['**csp:** Correct CSP asset protocol handling so local assets such as background images load reliably.'],
      },
      {
        title: 'Performance',
        items: ['**appearance:** Load system fonts asynchronously to keep the Appearance tab responsive.'],
      },
    ],
  },
  {
    version: '[1.0.6] - 2026-05-21',
    sections: [
      {
        title: 'Added',
        items: [
          '**appearance:** Add `Background Image` customization for the main window, including `Image Sizing`, `Image Opacity`, and `Background Content Opacity` controls.',
          '**sessions:** Add `Backspace Mode` selection for Telnet and Serial sessions with `Ctrl+H (BS)` and `DEL (0x7F)` options.',
        ],
      },
      {
        title: 'Changed',
        items: ['**resource-monitor:** Refresh the resource monitor with clearer cards and improved visual hierarchy.'],
      },
      {
        title: 'Fixed',
        items: [
          '**shortcuts:** Prevent Shift-modified terminal input from being mistaken for application shortcuts, restoring uppercase input such as `Shift+C`, `Shift+V`, and `Shift+X`.',
        ],
      },
    ],
  },
  {
    version: '[1.0.5] - 2026-05-19',
    sections: [
      {
        title: 'Added',
        items: [
          '**ai:** Capture AI command execution events and render inline terminal output during agent-driven workflows.',
          '**ai:** Add the `Terminal Output Lines` setting to control how many inline output lines are shown for AI-executed commands.',
          '**terminal:** Add AI Execution Profile selection to terminal session forms.',
          '**window:** Improve main-window modal management so child windows keep the workspace in a clearer modal state.',
        ],
      },
    ],
  },
  {
    version: '[1.0.4] - 2026-05-19',
    sections: [
      {
        title: 'Added',
        items: [
          '**cloud_sync:** Implement cloud synchronization features with encryption and logging.',
          '**ai:** Introduce AgentApprovalManager and refactor AI command handling.',
          '**session-management:** Enhance session management by adding initialGroupId support.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**ai:** Remove risk assessment features and AiRiskLevel from AI components and configuration.',
          '**i18n:** Update localization files for agent command execution and approval messages.',
          '**window:** Improve always-on-top logic for modal windows.',
          '**file-explorer:** Optimize file drop handling.',
          '**sftp:** Consolidate SFTP handling by removing SSH namespace and introducing new backend structure.',
          '**ssh-form:** Update SshForm and NewSessionPage for improved password handling and connection management.',
          '**cleanup:** Remove unused components and functions from AiTab and XTerminal.',
        ],
      },
    ],
  },
  {
    version: '[1.0.3] - 2026-05-18',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal:** Implement tab movement and unsplit functionality in terminal management.',
          '**zmodem:** Implement Zmodem file transfer commands, detection, and event handling.',
          '**shortcuts:** Implement customizable keyboard shortcuts and settings management.',
          '**window:** Enhance modal child window handling.',
          '**ai:** Enhance AI Assistant Panel with improved empty state handling.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**appearance:** Refactor font management and update default font settings.',
          '**i18n:** Add Zmodem transfer messages, terminal font family settings, keybindings, and AI setup instructions to localization files.',
          '**accessibility:** Add DialogDescription component to various dialogs.',
          '**theme:** Update color values in githubDark theme for improved consistency.',
        ],
      },
    ],
  },
  {
    version: '[1.0.2] - 2026-05-17',
    sections: [
      {
        title: 'Added',
        items: [
          '**credentials:** Implement credential management features with dialog and terminal autofill support.',
          '**security:** Enhance Password Management with Secret Unlocking Functionality and password reveal functionality.',
          '**terminal:** Add keyword highlighting settings and functionality in TerminalTab component.',
          '**clipboard:** Add CopyButton component for clipboard functionality.',
          '**tabbar:** Enhance TabBar component with dynamic tab visibility management and overflow handling.',
          '**templates:** Add issue templates for bug reports and feature requests.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**theme:** Update theme colors and CSS variables for improved consistency.',
          '**i18n:** Add localization entries for password management, search functionality, and hidden sessions.',
          '**ui:** Add required field indicators and improve label styling in session forms.',
          '**terminal:** Enhance ActionLinksAddon and KeywordHighlighter with improved timer management and refresh logic.',
          '**docs:** Enhance AI Assistant and security features in documentation.',
        ],
      },
    ],
  },
  {
    version: '[1.0.1] - 2026-05-16',
    sections: [
      {
        title: 'Added',
        items: [
          '**highlighting:** Add prompt highlighting support in keywordHighlightPresets.',
          '**app:** Integrate single instance support in Tauri application.',
          '**docs:** Add Umami analytics plugin to Docusaurus configuration.',
          '**ci:** Add GitHub Actions workflow for R2 asset publishing.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**file-explorer:** Clean up FileExplorer component by removing unused code and optimizing state management.',
          '**upload:** Simplify file upload handling in AutoUploadPage.',
          '**highlighting:** Update operator colors and regex pattern for keyword highlighting.',
          '**docs:** Update changelog with new releases and enhancements.',
          '**ci:** Update Docusaurus dependencies and remove optional Umami config.',
        ],
      },
    ],
  },

  {
    version: '[1.0.0] - 2026-05-06',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal-ai:** Add AI output capture in XTerminal with marker-based command execution capture.',
          '**connections:** Enhance connection management with recent connection tracking and matching localization strings.',
          '**downloads:** Enhance download platform management with architecture support and dynamic release asset fetching.',
          '**release:** Add Cloudflare R2 publishing and GitHub Actions workflows for release asset publishing.',
          '**branding:** Update the NyaTerm logo SVG with a new gradient and eye cutout mask.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**shell:** Remove PowerShell support from ShellKind and related shell handling logic.',
          '**branding:** Replace Dragonfly references with NyaTerm across documentation and the codebase.',
          '**updater:** Update the Tauri updater endpoint for improved version fetching.',
          '**deps:** Add strip-ansi-escapes and vte dependencies for more reliable terminal output handling.',
          '**ci:** Clean up obsolete debug publishing workflows.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**workflow:** Download GitHub Release assets during the publishing workflow.',
          '**workflow:** Add the TAG environment variable to the build-release workflow.',
        ],
      },
      {
        title: 'Documentation',
        items: ['**homepage:** Update home page images for dark and light themes.'],
      },
    ],
  },
  {
    version: '[0.9.0] - 2026-04-30',
    sections: [
      {
        title: 'Added',
        items: [
          '**ai-assistant:** Integrate the AI Assistant into the application, including terminal and file explorer actions, session history search, grouped sessions, copy selection, and session deletion.',
          '**agent:** Add agent mode with command execution, max step and timeout settings, command risk assessment, critical chmod/chown patterns, and a syntax-highlighted step view.',
          '**ai-chat:** Enhance AI chat streaming with session handling, cleanup, reasoning content, markdown support, structured output parsing, and improved logging.',
          '**storage:** Implement redb-based user data storage with JSON document updates, legacy migration improvements, and remote file reading.',
          '**macos:** Add macOS configuration and platform-specific header, child window, and layout adjustments.',
          '**update-dialog:** Render release notes as Markdown in the update dialog.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**app-layout:** Restructure the App component, introduce layout components, and streamline AppPanelContent panel rendering.',
          '**ai-settings:** Expand AI model listing and settings, simplify file-size settings, sort grouped models, and add AI localization updates.',
          '**ssh-form:** Refactor SshForm into tabs for proxy, jump host, and two-factor authentication settings.',
          '**ui:** Improve AIAssistantPanel, ModelCombobox, QuickCommands, action tooltips, and thinking text styling.',
          '**deps:** Add react-markdown, remark-gfm, react-syntax-highlighter, browserslist, lightningcss, and related dependency updates.',
          '**codebase:** Clean up formatting, import ordering, and function signatures across multiple modules.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**dialogs:** Add cleanup handling for dialog and alert dialog overlays.',
          '**ai-assistant:** Improve truncate_preview string truncation and remove the toast notification for text selection.',
          '**macos:** Correct titleBarStyle casing in the macOS configuration file.',
          '**ssh-form:** Adjust SshForm formatting and dialog import ordering.',
        ],
      },
      {
        title: 'Documentation',
        items: [
          'Update configuration storage documentation for the redb-backed data model.',
          'Expand documentation to include AI Assistant features and updates.',
        ],
      },
    ],
  },
  {
    version: '[0.8.5] - 2026-04-28',
    sections: [
      {
        title: 'Added',
        items: [
          '**session-sync:** Implement session synchronization support.',
          '**quick-commands:** Add support for sending commands to all users from QuickCommands.',
          '**release:** Add workflows to repair latest.json and release updater assets.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**ci:** Update the build-release workflow, repair asset download scripts, and release asset publishing flow.',
          '**docs:** Update the homepage URL and add a documentation page link to the header menu.',
          '**i18n:** Add synchronization group features and menu option strings in English and Chinese.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**ci:** Enhance build-release workflow cache cleanup, add libudev-dev to build dependencies, and fix GITHUB_TOKEN indentation.',
          '**updater:** Add Tauri updater signing key preparation and improve updater manifest generation.',
        ],
      },
    ],
  },
  {
    version: '[0.8.4] - 2026-04-27',
    sections: [
      {
        title: 'Added',
        items: [
          '**ssh:** Implement HostKeyVerifyManager for host key verification and known_hosts management.',
          '**ssh:** Enhance host key verification logging and add a timeout for verification.',
        ],
      },
      {
        title: 'Changed',
        items: ['**i18n:** Add SSH host key verification messages in English and Chinese locales.'],
      },
      {
        title: 'Fixed',
        items: ['**host-key-verification:** Add HostKeyVerifyDialog and integrate host key verification handling in the app.'],
      },
      {
        title: 'Documentation',
        items: ['Update Docusaurus configuration to handle broken anchors.'],
      },
    ],
  },
  {
    version: '[0.8.3] - 2026-04-27',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal:** Implement command suggestion visibility based on shell integration state and terminal mode.',
          '**file-explorer:** Add a parent directory entry and update context menu behavior for smoother navigation.',
        ],
      },
      {
        title: 'Changed',
        items: ['**resource-monitor:** Enhance the resource monitor UI and improve performance metric formatting.'],
      },
      {
        title: 'Documentation',
        items: ['Add CHANGELOG.md to document notable changes for version 0.8.2.'],
      },
    ],
  },
  {
    version: '[0.8.2] - 2026-04-23',
    sections: [
      {
        title: 'Added',
        items: [
          '**tauri:** Add Windows configuration file and remove unused dragDropEnabled property.',
          '**file-transfer:** Enhance file transfer handling to support directories, including progress tracking and UI updates for directory transfers.',
          '**session-management:** Implement session-specific command history management, including fetching, listening, and clearing command history for improved user experience.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**i18n:** Add new file transfer messages for progress tracking and completion in English and Chinese locales.',
          '**header:** Update window control buttons with new icons and improved styling for better user experience.',
        ],
      },
      {
        title: 'Fixed',
        items: ['**saved-connections:** Implement drag-and-drop support for connection and group items, enhancing user interaction and organization.'],
      },
      {
        title: 'Performance',
        items: ['**file-explorer:** Enhance FileExplorer component with memoization and scroll handling for improved performance and user experience.'],
      },
      {
        title: 'Documentation',
        items: [
          'Update README and guides to include new features such as Windows drag-and-drop support, enhanced file transfer capabilities, and diagnostics settings for improved user experience.',
          '**file-transfer:** Refine drag-and-drop upload section for clarity and consistency across languages.',
        ],
      },
    ],
  },
  {
    version: '[0.8.1] - 2026-04-23',
    sections: [
      {
        title: 'Added',
        items: [
          '**interaction:** Add command suggestion min character limit settings and normalization logic for enhanced user control.',
          '**file-explorer:** Implement external file drop support on Windows using WebView2 for enhanced drag-and-drop functionality.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**i18n:** Add command suggestions min character limit settings to English and Chinese locales for enhanced user control.',
          '**file-transfer:** Optimize visibleTransfers calculation using useMemo for improved performance and sorting.',
          '**terminal:** Replace useApp with useTerminalAppSettings for improved settings management and consistency across terminal components.',
          '**sync-backup:** Update button size from icon-xs to icon-sm for improved UI consistency.',
          '**i18n:** Add external drop support messages for English and Chinese locales to improve user guidance during file uploads.',
        ],
      },
      {
        title: 'Documentation',
        items: ['Enhance documentation with new features including session import/export, diagnostics, and tray support for improved user experience and clarity.'],
      },
    ],
  },
  {
    version: '[0.8.0] - 2026-04-22',
    sections: [
      {
        title: 'Added',
        items: [
          '**interaction:** Add command suggestion max character limit settings and normalization logic for improved user control over command suggestions.',
          '**quit_confirmation:** Implement QuitConfirmDialog for user confirmation before application exit, enhancing user experience and preventing accidental closures.',
          '**tray:** Implement tray functionality with window management and application quit command for enhanced user experience.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**i18n:** Add command suggestions max character limit settings to English and Chinese locales for improved user control.',
          '**syncbackup:** Enhance SyncBackupHistoryPanel with new UI components, improved history summary logic, and additional filtering options for better user experience.',
          '**i18n:** Add new history-related terms to English and Chinese locales for improved user experience and clarity.',
          '**scrollbar:** Add transparent background for scrollbar corner to improve UI consistency.',
          '**saved-connections:** Update layout and styling for improved responsiveness and visual consistency.',
          '**settings:** Remove emit calls for settings changes in ChildAppProvider and SettingsPage to streamline event handling.',
        ],
      },
    ],
  },
  {
    version: '[0.7.9] - 2026-04-21',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal:** Enhance terminal input handling by synchronizing input state from rendered lines and improving command processing logic.',
          '**syncbackup:** Implement SyncBackup functionality with UI components for managing cloud sync settings and history, enhancing user experience for backup management.',
          '**security:** Add master password management with dynamic state handling and improve input components for better user experience.',
          '**syncbackup:** Add validation for S3 endpoint requirement and improve UI feedback for draft settings, enhancing user experience in cloud sync management.',
          '**otp:** Integrate input-otp component for enhanced OTP input handling in OtpDialog, improving user experience with dynamic code length management.',
          '**cloud_sync:** Enhance error handling for WebDAV authentication by adding specific messaging for 401 errors and improving storage error mapping.',
          '**syncbackup:** Enhance SyncBackupHistoryPanel with filtering capabilities, improved state management, and UI updates for better user experience.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**terminal:** Remove unused input synchronization logic and streamline command sanitization process.',
          '**terminal:** Rename command tracking function and enhance command registration logic for improved input handling.',
          '**i18n:** Update English and Chinese locale files with new strings for sync and backup features, enhancing user interface and experience.',
          '**settings:** Restructure settings page with categorized groups, improved scroll handling, and dynamic tab management for enhanced user experience.',
          '**i18n:** Update zh-CN locale with new sync and backup history terms, enhance filtering options, and improve user prompts.',
        ],
      },
      {
        title: 'Fixed',
        items: ['**file-explorer:** Implement session caching for file explorer to maintain state across unmounts, enhancing user experience during navigation.'],
      },
      {
        title: 'Documentation',
        items: ['Enhance documentation and UI for Sync & Backup features, including detailed guides, settings integration, and improved user experience for cross-device configuration and backup management.'],
      },
    ],
  },
  {
    version: '[0.7.8] - 2026-04-21',
    sections: [
      {
        title: 'Added',
        items: [
          '**shell:** Implement command sanitization for terminal input and add terminal command utility functions.',
          '**session:** Refactor session input handling by introducing sendSessionInput function for improved command submission and preview management across components.',
          '**logging:** Introduce console usage linting and enhance error logging structure across components for improved diagnostics.',
          '**keywordhighlight:** Expand error and control flow patterns in keyword highlighting for enhanced diagnostics.',
          '**quickcommands:** Implement QuickCommandsStore for managing quick commands with in-memory caching and persistence, enhancing command upsert and retrieval functionality.',
        ],
      },
      {
        title: 'Changed',
        items: ['**observability, watcher, auth:** Apply consistent formatting and indentation across multiple functions for improved code readability.'],
      },
      {
        title: 'Performance',
        items: ['Optimize context providers by utilizing useMemo for context values in AppContext, ChildAppProvider, and TransferProvider.'],
      },
    ],
  },
  {
    version: '[0.7.7] - 2026-04-15',
    sections: [
      {
        title: 'Added',
        items: [
          'Implement import/export configuration functionality with UI updates in ImportDialog and Header components.',
          '**backup:** Add import/export functionality for configuration with encryption and rotation.',
          '**connections:** Add OpenGroupConnectionsDialog component and enhance connection item interactions with selection and context menu options.',
          '**panel:** Enhance QuickCommands component with improved search and category filtering UI.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**i18n:** Update English and Chinese translations for configuration import/export features and add new UI strings.',
          '**panel:** Update ActiveSessions component with improved styling for search input and icon.',
          '**panel:** Adjust width of dropdown menu in SavedConnections component for better UI consistency.',
        ],
      },
    ],
  },
  {
    version: '[0.7.6] - 2026-04-15',
    sections: [
      {
        title: 'Added',
        items: [
          '**ssh:** Improve SSH authentication logging and add known host key verification.',
          '**ssh:** Enhance SSH I/O loop with detailed exit status and signal logging.',
        ],
      },
      {
        title: 'Changed',
        items: ["Add 'des' crate dependency to Cargo.toml and update Cargo.lock."],
      },
      {
        title: 'Fixed',
        items: ['Restore import of SessionOutputCoalescer in pty.rs for proper session output handling.'],
      },
      {
        title: 'Documentation',
        items: [
          'Update README with new features and enhancements including online search, translation, and improved SFTP file explorer.',
          'Enhance documentation with updates on terminal features, file transfer capabilities, and security enhancements including translation support and improved session management.',
        ],
      },
    ],
  },
  {
    version: '[0.7.5] - 2026-04-14',
    sections: [
      {
        title: 'Added',
        items: [
          '**connection:** Enhance session connection handling with improved error recovery and connection editing prompts.',
          '**ssh:** Enhance SSH form with password management and localization updates.',
        ],
      },
    ],
  },
  {
    version: '[0.7.4] - 2026-04-14',
    sections: [
      {
        title: 'Added',
        items: [
          '**updater:** Implement update dialog and background update check functionality.',
          '**header:** Enhance header component with update check functionality and new icons.',
          '**terminal:** Add suspended state handling to terminal components and output coalescing for improved performance under load.',
        ],
      },
      {
        title: 'Changed',
        items: [
          'Add @tauri-apps/plugin-process and @tauri-apps/plugin-updater dependencies to package.json and pnpm-lock.yaml.',
          'Clean up imports and improve formatting across multiple components for better readability.',
          '**i18n:** Add updater localization for English and Chinese, including update status messages.',
          '**i18n:** Add localization for large output protection messages in English and Chinese.',
        ],
      },
    ],
  },
  {
    version: '[0.7.3] - 2026-04-14',
    sections: [
      {
        title: 'Added',
        items: [
          '**keywordhighlightpresets:** Expand success patterns to include additional keywords for improved matching.',
          '**connection-management:** Implement error handling for connection failures, adding support for marking tabs and panes as failed while maintaining layout integrity.',
          '**file-explorer:** Implement directory history management and enhance selection handling.',
          '**file-transfer:** Add pause, resume, and cancel functionality for file transfers with updated context and UI components.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**i18n:** Add connection failure messages in English and Chinese localization.',
          '**file-explorer:** Update selection handling methods and improve context menu interactions.',
          '**i18n:** Update English and Chinese localization for file transfer actions including cancel, pause, resume, and delete.',
        ],
      },
    ],
  },
  {
    version: '[0.7.2] - 2026-04-14',
    sections: [
      {
        title: 'Added',
        items: [
          '**interaction-settings:** Add command suggestions toggle to InteractionTab and integrate with app settings.',
          '**logging:** Implement persistent logging for warn and error levels, add Tauri command to handle log writing.',
          '**file-explorer:** Enhance keyboard interaction by adding delete functionality and focus management for the file list.',
          '**sftp:** Enhance remote file operations with detailed logging and permission handling.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**file-explorer:** Replace invoke import with local library and add autoFocus to delete button for improved accessibility.',
          '**file-explorer:** Replace invoke import with local library across multiple dialog components for consistency.',
          '**i18n:** Add command suggestions localization in English and Chinese.',
        ],
      },
      {
        title: 'Fixed',
        items: ['**keywordhighlightpresets:** Update duration regex to include shorthand units for better matching.'],
      },
      {
        title: 'Documentation',
        items: ['Update CLAUDE.md and README.md to clarify commands for building and serving the docs site, including locale-specific hot reload options.'],
      },
    ],
  },
  {
    version: '[0.7.1] - 2026-04-13',
    sections: [
      {
        title: 'Added',
        items: [
          '**clipboard:** Implement readClipboardText function and update terminal components to use it for clipboard access.',
          '**demos:** Add various demo scripts for showcasing NyaTerm\'s terminal features, including action links, file watching, keyword highlighting, and structured output.',
          '**activesessions:** Enhance ActiveSessions component with search functionality, session reconnect/disconnect actions, and improved UI for session display.',
          '**file-explorer:** Refactor DeleteDialog to handle multiple file deletions and improve UI; update FileExplorer to support batch delete actions.',
          '**resource-monitor:** Implement refresh button and improve stats fetching with async/await; add loading state management.',
          '**modal-management:** Refactor modal child window handling to improve focus enforcement and state tracking; add reconnect and disconnect session functionality in ActiveSessions component.',
          '**activesessions:** Simplify PanelHeader actions by removing unnecessary wrapper div for session count display.',
          '**resource-monitor:** Enhance refresh button with tooltip and rename state variable for clarity.',
        ],
      },
      {
        title: 'Changed',
        items: ['**i18n:** Update zh-CN and en.json for activeSessions and file deletion messages.'],
      },
      {
        title: 'Documentation',
        items: [
          'Update README and user guides to enhance clarity on NyaTerm\'s features, session types, and terminal capabilities; add new sections for workspace layout, security, and network configurations.',
          '**sidebars:** Update guide sections to include new topics on session types, layout, and authentication while reorganizing existing items for better clarity.',
        ],
      },
    ],
  },
  {
    version: '[0.7.0] - 2026-04-12',
    sections: [
      {
        title: 'Added',
        items: [
          'Enhance terminal workspace with new tab management and pane functionality.',
          '**crypto:** Implement master password wrapping key cryptosystem.',
          '**app:** Restore cryptographic master password state on app startup.',
          '**config:** Introduce proxy_jump_id field and circular-dependency validation.',
          '**ssh:** Implement multi-hop proxy jump routing via direct-tcpip channel.',
          '**ui:** Integrate jump host configuration into SSH session dialog.',
          '**shell:** Upgrade serial sender into unified shell command broadcaster.',
          '**explorer:** Restrict file explorer to SSH sessions and show unsupported message.',
          '**tabbar:** Add unread indicator with breathing animation and extend TabBarProps.',
          '**unreadtracking:** Implement unread session output tracking and update TabWindowsWorkspace to display unread tab IDs.',
          '**terminal:** Add TerminalGutter component for displaying line numbers and timestamps; update settings to disable action links by default.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**window:** Enable transparent window background in tauri config.',
          '**ssh:** Reduce default keepalive interval from 60s to 3s.',
          '**config:** Format ui configuration tuple structures.',
          '**security:** Migrate lock_password to unified master_password definitions.',
          '**ssh:** Decouple single session handle into multi-tiered SshConnectionHandles.',
          '**panel:** Migrate QuickCommands and SerialSendPanel to panel module.',
          '**ui:** Remove legacy fullscreen shortcuts and redundant menu entries.',
          '**panel:** Adjust active sessions count indicator formatting.',
          'Commit remaining changes.',
          '**keywordhighlight:** Update token boundary handling to remove conflicts.',
          '**i18n:** Add line numbers and timestamps options to terminal settings.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**otp:** Properly decode multi-byte utf-8 characters in url encoding.',
          '**ssh:** Prevent prompt injection scripts from polluting shell history.',
          '**session:** Silently ignore not-found error during session close.',
          '**terminal:** Suppress errors when attaching to terminating sessions.',
          '**terminal:** Prevent dismissing suggestions when there are no active suggestions or selection.',
          '**settings:** Disable keyword highlights and action links by default in terminal settings.',
        ],
      },
      {
        title: 'Performance',
        items: [
          'Only remove workspace tabs from UI after successful close.',
          'Make split-window session placement explicit.',
          'Reduce unnecessary re-renders in terminal workspace.',
        ],
      },
      {
        title: 'Documentation',
        items: ['Add CLAUDE.md for development guidance and architecture overview.'],
      },
    ],
  },
  {
    version: '[0.6.1] - 2026-04-11',
    sections: [
      {
        title: 'Changed',
        items: ['Update version synchronization in sync-version script.', 'Update nyaterm dependency version to 0.6.0.'],
      },
    ],
  },
  {
    version: '[0.6.0] - 2026-04-11',
    sections: [
      {
        title: 'Added',
        items: [
          '**proxy:** Standalone proxy and tunnel management.',
          '**sftp:** Enhance file transfer with concurrency, retries, and timestamps.',
          '**ui:** Implement network panel and settings restructuring.',
          'Implement Tauri commands for secure app settings management and password verification.',
          '**network:** Enhance tunnel configuration UI.',
          'Add session recording and custom transfer preferences.',
          '**ui:** Add OtpDialog for two-factor authentication support.',
          '**core:** Implement OTP interaction with PendingAuthManager and commands.',
          '**ui:** Implement OSC7 CWD tracking support and UI disabled states.',
          '**ui:** Integrate OtpDialog into main app layout with i18n support.',
          '**transfer:** Open download path from transfer footer.',
          '**security:** Add tab count display and update Key/Password management tabs to report counts.',
          '**ssh-form:** Enhance SSH form with proxy and OTP configuration options.',
          '**otp:** Implement OTP management and integration with UI components.',
          '**prettier:** Add Prettier configuration for JSON sorting and update package scripts for i18n checks.',
          '**search:** Add show_in_menu property to SearchEngine and enhance SearchTab with collapsible UI for custom engines.',
          '**session:** Launch local, telnet, and serial connections by type.',
          '**serial:** Show detected serial ports in the session editor.',
          '**serial:** Add bottom serial send panel.',
        ],
      },
      {
        title: 'Changed',
        items: [
          '**ui:** Introduce shadcn UI components.',
          '**i18n:** Update translations for network and transfer features.',
          '**translate:** Minor module dependency updates for translate API.',
          'Format session proxy imports.',
          'Adjust panel header actions layout.',
          '**deps:** Bump russh to 0.60.',
          '**ui:** Rename saved-connections dialog directory to connections.',
          '**core:** Reorganize module structure for ssh, runtime, and import.',
          'Update internal imports and finalize ssh module extraction.',
          '**core:** Adopt new ssh and runtime module structures.',
          '**ui:** Update import path in Header for new connections directory.',
          'Restructure command modules and update import paths for improved organization.',
          '**config:** Rename storage modules and split settings config.',
          '**runtime:** Extract tauri bootstrap and command adapters.',
          '**core:** Extract history store and unify error imports.',
          '**session-dialog:** Make new session forms responsive.',
          '**dialog:** Improve quick command and auto upload layouts.',
          '**settings:** Introduce responsive settings shell.',
          '**settings-search:** Reflow custom search engine editor.',
          '**settings-terminal:** Reflow action link and highlight editors.',
          '**panel:** Polish mobile panels and auth tabs.',
          '**core:** Export watcher module.',
          '**rust:** Normalize backend formatting.',
          '**i18n:** Normalize english sort labels.',
          '**otp:** Vendor local hotp and totp crate.',
          '**format:** Remove trailing whitespace artifacts.',
          '**format:** Trim trailing blank line in translate core.',
          '**quick-commands:** Clean up formatting and improve tooltip component structure.',
          '**resource-monitor:** Improve code formatting and structure for better readability.',
          '**settings:** Restructure settings components to use SettingSection for better organization and readability.',
          'Reorganize file explorer, auth and save-connections components.',
          '**connection:** Normalize saved connection schema into typed config blocks.',
          '**saved-connections:** Extract tooltip-backed header action button.',
          '**file-explorer:** Reuse tooltip icon buttons in the toolbar.',
          '**i18n:** Remove deprecated default local shell labels.',
          '**frontend:** Normalize panel imports and minor cleanup.',
          '**rust:** Isolate import reordering and line-wrap churn.',
          '**file-explorer:** Wrap dialog import for consistency.',
          'Introduce FileUploadPage and update routing to replace AutoUploadPage.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**ssh:** Resolve concurrent SshHandler access using Mutex.',
          '**security:** Add app scope to temp dir capabilities.',
          '**ui:** Handle xterm buffer trimming in keyword highlighter cache.',
          '**i18n:** Correct Chinese translations for various UI strings.',
          '**explorer:** Normalize cwd paths before syncing directories.',
          '**panel:** Update default tab in SecurityAuthPanel from passwords to keys.',
          '**ssh:** Use character escapes for PowerShell OSC integration.',
          '**select:** Allow trigger content to shrink and truncate in narrow layouts.',
          '**session-ui:** Restrict SSH-only panels and clarify path-sync messaging.',
          '**session-editor:** Reset local terminal defaults when clearing the form.',
          '**i18n:** Update serial port messages and reintroduce serial send localization.',
        ],
      },
    ],
  },
  {
    version: '[0.5.0] - 2026-04-07',
    sections: [
      {
        title: 'Added',
        items: [
          '**window:** Implement child window modal management and overlay.',
          '**auth:** Add managed password store for SSH sessions.',
          '**stats:** Add remote resource monitor for SSH sessions.',
          '**sftp:** Add recursive directory transfer commands.',
        ],
      },
      {
        title: 'Changed',
        items: ['Update styling for tab borders and shadows.', '**ui:** Adopt activity bar layout and custom window chrome.'],
      },
      {
        title: 'Fixed',
        items: ['**i18n:** Refine experimental keyword highlighting description in Chinese locale.', '**terminal:** Reconnect SSH tabs after disconnect.'],
      },
    ],
  },
  {
    version: '[0.4.0] - 2026-04-03',
    sections: [
      {
        title: 'Added',
        items: [
          'Implement ChildWindowRouter and enhance window management with i18n support.',
          'Enhance keyword highlighting settings and functionality.',
          'Update word separators in interaction settings for improved parsing.',
          'Enhance file transfer functionality and loading state management.',
          'Add keyword highlight setting for wrapped lines in TerminalTab.',
          '**session:** Add multi-protocol tabs to new session form.',
          '**file-explorer:** Open auto-upload prompts in child windows.',
          '**appearance:** Support dedicated terminal themes and font scaling.',
          '**terminal:** Add actionable links and hover menus.',
        ],
      },
      {
        title: 'Changed',
        items: [
          'Update project URLs and enhance build script.',
          "**i18n:** Add 'Built-in' font label to English and Chinese translations.",
          '**ui:** Polish tab chrome and refresh connection icons.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**app:** Stabilize active tab state and terminal defaults.',
          '**keywordhighlight:** Improve built-in matching and cell mapping.',
          '**build:** Align Vite typing and path alias settings.',
        ],
      },
      {
        title: 'Documentation',
        items: ['Add Docusaurus documentation site with bilingual support.', 'Redesign homepage and fix i18n issues.'],
      },
    ],
  },
  {
    version: '[0.3.5] - 2026-03-09',
    sections: [
      {
        title: 'Fixed',
        items: ['**keywordhighlight:** Enhance datetime and number patterns for better matching accuracy.'],
      },
    ],
  },
  {
    version: '[0.3.4] - 2026-03-09',
    sections: [
      {
        title: 'Changed',
        items: ['**terminal:** Replace kbd elements with Kbd component for consistency in CommandSuggestions and ContextMenu.'],
      },
    ],
  },
  {
    version: '[0.3.3] - 2026-03-09',
    sections: [
      {
        title: 'Added',
        items: [
          '**terminal:** Add keyword highlighting feature.',
          '**connections:** Add edit option to connection item context menu.',
          '**settings:** Support navigating to specific settings tab and auto-refresh ssh keys on focus.',
          '**shortcuts:** Implement global keyboard shortcuts for terminal and UI actions.',
        ],
      },
      {
        title: 'Changed',
        items: [
          'Sync version in Cargo.lock and update commit files list.',
          '**terminal:** Improve TabBar close button UI and hover states.',
          '**terminal:** Use React.RefObject instead of MutableRefObject for terminal refs.',
          '**theme:** Update terminal cursor colors for githubLight and nordLight themes.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          '**terminal:** Re-initialize WebGL addon on hardware acceleration toggle.',
          '**ssh:** Prevent OSC 7 injection from polluting bash history.',
        ],
      },
    ],
  },
  {
    version: '[0.2.1] - 2026-03-06',
    sections: [
      {
        title: 'Added',
        items: [
          '**session-management:** Enhance session handling with auto-connect feature.',
          '**types:** Add comprehensive global types for session management and UI configuration.',
          '**file-explorer:** Add dialogs for creating new files, folders, and symlinks.',
          '**translate:** Implement dynamic TKK generation for Google Translate.',
          '**file-explorer:** Implement terminal path synchronization feature.',
        ],
      },
      {
        title: 'Changed',
        items: [
          'Relocate themes and types to lib directory.',
          'Update `.gitignore` to include additional file patterns.',
          'Update import paths and enhance translation settings.',
          'Update import paths to global types.',
          '**icons:** Consolidate file icon logic and enhance icon imports.',
        ],
      },
    ],
  },
  {
    version: '[0.1.5] - 2026-03-06',
    sections: [
      {
        title: 'Added',
        items: [
          '**ui:** Implement zoom level persistence and view settings.',
          '**ui:** Add clickable homepage and issues links to about dialog.',
          '**ui:** Enhance header menu with icons and new Help options for documentation and logs.',
          '**logging:** Enhance tracing initialization with rolling file appender and update log permissions.',
          '**window:** Show application window on startup and update tauri configuration to allow window visibility.',
          '**connections:** Add the SavedConnections panel for grouped SSH connection management.',
          '**watcher:** Add file watcher support and chunked file transfer progress tracking.',
          '**file-explorer:** Integrate custom dialogs and context menu support.',
          '**settings:** Implement global settings dialog and localization.',
          '**terminal:** Add terminal context menu utilities and search bar.',
          '**security:** Add lock screen and lock password encryption.',
          '**quick-commands:** Redesign quick commands UI with icons and variables support.',
          '**file-transfer:** Add file properties dialog and transfer progress bar.',
          '**settings:** Add translation settings and a tabbed settings/about experience.',
          '**translate:** Add TranslationTab and multi-provider translation service.',
          '**terminal:** Enhance XTerminal with URL opening and better command history handling.',
          '**app:** Introduce global application context and broader i18n support.',
          '**search:** Add search engine icons and improve search tab configuration UI.',
          '**import:** Add session import from Xshell, MobaXterm, and WindTerm.',
          '**ui:** Add command palette, popover, and draggable panel components.',
          '**icons:** Expand the icon system and update type definitions.',
          '**connections:** Enhance connection handling, feedback, sorting, and drag-and-drop.',
          '**config:** Add screen lock and connection sort mode settings.',
          '**security:** Implement screen lock toggle and idle detection.',
          '**suggestions:** Enhance command suggestions with multi-provider support.',
          '**event-listeners:** Replace polling with event listeners for session and command history updates.',
        ],
      },
      {
        title: 'Changed',
        items: [
          'Add MIT License file.',
          '**assets:** Update app icons, logo assets, and remove unused SVGs.',
          '**cleanup:** Update tauri config and remove unused assets.',
          '**i18n:** Integrate i18next across the application.',
          '**ui:** Update page title from `NyaTerm Terminal` to `NyaTerm`.',
          'Update scrollbar styling.',
          'Update global UI, layout visibility, and theme configuration.',
          'Adopt shadcn/ui components.',
          'Migrate toast notifications to sonner and use shadcn context menus.',
          'Update settings dialog to use switches and a tabbed interface.',
          '**i18n:** Update localizations for new components and features.',
          'Update typography, CSS variables, theme colors, and section headers.',
          'Update dependencies, shared utils, types, UI components, and panels.',
          '**backend:** Modularize config and commands into submodules.',
          '**theme:** Overhaul the theme system with CSS variables and preset themes.',
          '**dialog:** Reorganize dialogs into domain-specific subdirectories.',
          '**settings:** Update settings tabs for the new config structure.',
          '**app:** Refresh App, contexts, layout, and panel components.',
          '**i18n:** Add locale keys for newly introduced settings and UI flows.',
          '**window:** Migrate dialogs to independent child windows.',
          '**file-explorer:** Modularize the file tree and replace native dialogs.',
          '**terminal:** Clean up formatting and whitespace issues.',
          '**tracing:** Improve local time formatting and remove inline key migration.',
          '**dialogs:** Remove NewSessionDialog, SettingsDialog, and QuickCommandDialog.',
          '**components:** Extract settings components and standardize import paths.',
          'Delete generated build output from the repository.',
          'Bump version to `0.1.5` and add version synchronization script.',
        ],
      },
      {
        title: 'Fixed',
        items: [
          'Resolve dialog accessibility warnings.',
          'Update translation key usage in SearchTab for clearer settings descriptions.',
          'Improve session handling and UI responsiveness.',
          '**settings:** Update default interaction settings for copy and paste.',
          '**translations:** Remove fallback values from translation keys in dialogs and components.',
        ],
      },
      {
        title: 'Performance',
        items: ['**sftp,ssh:** Optimize transfer speeds and add symlink support.'],
      },
      {
        title: 'Documentation',
        items: ['Update README with key features and usage instructions.', 'Remove the trailing period from the README tagline.'],
      },
    ],
  },
];

const changelogReleasesZhCN: ChangelogRelease[] = [
  {
    version: '[1.2.8] - 2026-09-03',
    sections: [
      {
        title: '新增',
        items: [
          '**ssh:** 从远程命令历史中清理 NyaTerm 注入的 Bash Shell 集成命令，并兼容已禁用 `cmdhist` 的 Shell。',
        ],
      },
      {
        title: '变更',
        items: [
          '**editor:** 使用主题化控件、焦点环和复选框强调色重新设计内置编辑器的搜索面板。',
          '**ssh:** 将 Shell 集成注入脚本的单行长度控制在远程 Linux PTY 规范输入限制内。',
        ],
      },
    ],
  },
  {
    version: '[1.2.7] - 2026-09-02',
    sections: [
      {
        title: '新增',
        items: [
          '**editor:** 内置文件编辑器新增 Ctrl+鼠标滚轮缩放字体。',
          '**quick-commands:** 终端上下文菜单新增“将选中文本保存为快捷命令”操作。',
          '**terminal:** 新增动态终端标签标题。',
          '**sftp:** 新增目录列表和文件状态命令的共用辅助函数。',
          '**ai:** 改进 AI 流控制事件处理，并补充 MCP 错误的本地化文案。',
        ],
      },
      {
        title: '修复',
        items: [
          '**file-drop:** 仅接受最上层的有效放置目标，改进嵌套放置区域的识别。',
          '**quick-commands:** 保存快捷命令时保留终端选区。',
          '**terminal:** 存在选区时输入会滚动到光标，并在切换标签后恢复终端焦点。',
          '**terminal:** 休眠会话唤醒失败时恢复标签标题。',
          '**rdp:** 编辑 RDP 连接时保留已配置的跳板机。',
        ],
      },
    ],
  },
  {
    version: '[1.2.6] - 2026-09-01',
    sections: [
      {
        title: '新增',
        items: [
          '**mcp:** 新增 MCP sidecar、活动会话上报、会话打开请求、权限模式和基于 Windows ACL 的发现机制。',
          '**security:** 将锁屏拆分为独立的启动锁定和空闲锁定设置。',
          '**ssh:** 新增支持 ProxyJump 和多跳连接的 SSH config 导入。',
          '**sftp:** 新增可配置的流水线深度和符号链接目标管理。',
          '**terminal:** 新增可配置的右键操作，并支持将选中文本保存为快捷命令。',
          '**asset-monitoring:** 改进资产增量更新和会话校验。',
        ],
      },
      {
        title: '变更',
        items: [
          '**terminal:** 精简上下文菜单，并改进终端刷新和关键词高亮效果。',
          '**dependencies:** 更新 Tauri 和 Rust 依赖，并为 MCP 发现功能加入 Windows 安全特性。',
          '**network:** 调整隧道行和运行状态徽章的布局与样式。',
          '**i18n:** 补充 MCP 权限、SFTP 流水线深度、终端操作、符号链接和锁定设置的多语言文案。',
        ],
      },
      {
        title: '修复',
        items: [
          '**clipboard:** 支持在终端中粘贴 Windows 剪贴板历史记录。',
          '**serial-send:** 串口发送面板隐藏再显示后保留输入内容。',
          '**editor:** 内置文件编辑器复用终端选区颜色。',
          '**ai:** 防止输入法组合期间按 Enter 意外提交 AI 提示词。',
        ],
      },
      {
        title: '性能',
        items: ['**terminal:** 新增输出确认协调、快照恢复，并提高终端刷新的可靠性。'],
      },
      {
        title: '文档',
        items: ['**development:** 记录项目使用的全部四个语言文件。'],
      },
    ],
  },
  {
    version: '[1.2.5] - 2026-08-24',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal:** 新增会话休眠与输出控制、图片渲染、前台帧回退和改进的后台输出排空。',
          '**credential-autofill:** 检测凭据提示并提供已保存凭据的自动填充。',
          '**file-explorer:** 自动与终端当前工作目录保持同步。',
          '**zmodem:** 改进上传冲突处理，并显示远端拒绝上传的原因。',
          '**send-command-panel:** 新增“所有窗口”目标和发送后清空串口输入选项。',
          '**settings:** 支持拖拽调整终端字体栈顺序。',
          '**windows-7:** 新增固定版 WebView2 Runtime 支持和专用构建流程。',
          '**ai-settings:** 新增 API 格式选择并改进模型服务商默认配置。',
          '**remote-desktop:** 为 RDP 和 VNC 新增代理与跳板机选择，并改进 RDP 错误报告。',
          '**custom-icons:** 新增自定义连接图标管理。',
          '**ssh:** 新增用于外部调用和协议链接的 SSH 运行模式。',
          '**activity-bar:** 新增可见性管理和重置操作。',
          '**floating-panels:** 新增浮动模式和面板状态持久化。',
          '**quick-commands:** 新增命令变量解析和交互式取值。',
          '**editor:** 新增可配置的内置/外部文件打开模式。',
          '**session:** 新增外部本地会话调用和工作目录校验。',
        ],
      },
      {
        title: '变更',
        items: [
          '**terminal:** 改进透明背景和事件监听器生命周期管理。',
          '**app:** 改进 macOS 子窗口生命周期处理并整合覆盖层对话框。',
          '**ai:** 改进会话历史请求跟踪和错误处理。',
          '**i18n:** 为新增设置和流程补充、更新多语言文案。',
        ],
      },
      {
        title: '修复',
        items: [
          '**terminal:** OSC 133 报告命令边界时重置命令运行状态。',
          '**webview:** 新增受保护的强制刷新快捷键处理。',
          '**ai:** 防止输入法组合期间按 Enter 意外提交提示词。',
        ],
      },
      {
        title: '文档',
        items: ['**ssh:** 记录本地调用、连接链接格式，以及 RDP/VNC 代理和跳板机支持。'],
      },
    ],
  },
  {
    version: '[1.2.4] - 2026-08-17',
    sections: [
      {
        title: '新增',
        items: [
          '**file-editor:** 新增文本编辑、未保存更改保护和安全关闭流程。',
          '**sftp:** 对文本文件进行编辑器打开分类，同时保留底层错误。',
          '**ssh:** 新增可配置的 SSH Agent 转发。',
          '**file-explorer:** 新增带校验和测试的移动对话框。',
          '**terminal-session:** 改进本地和 WSL 命令的工作目录处理。',
          '**child-window:** 新增 macOS 仅拖拽标题区域和可恢复的命令队列。',
        ],
      },
      {
        title: '修复',
        items: [
          '**saved-connections:** 防止重复提交文件夹。',
          '**context-menu:** 防止右键点击意外激活菜单项。',
          '**local-terminal:** 改进 Shell 路径输入行为和本地化。',
          '**macos:** 使子窗口红绿灯按钮对齐更加一致。',
        ],
      },
      {
        title: '性能',
        items: [
          '**terminal:** 新增备用屏幕跟踪、输出调度和 DEC 2026 帧门控。',
          '**window:** 减少子窗口启动工作量并缩短加载时间。',
        ],
      },
      {
        title: '文档',
        items: ['**ssh:** 记录外部调用和支持的协议链接。'],
      },
    ],
  },
  {
    version: '[1.2.3] - 2026-08-13',
    sections: [
      {
        title: '新增',
        items: [
          '**vnc:** 新增 VNC 会话，支持已保存连接、工作区集成、认证处理、剪贴板、输入转发和帧缓冲渲染。',
          '**remote-desktop:** 新增 VNC 和 RDP 面板共用的远程桌面帧、视口、渲染器和 Surface 工具。',
          '**terminal:** 新增会话录制和转录管理流程。',
          '**asset-management:** 新增连接时长跟踪、显示格式化、资产排序和排序状态持久化。',
          '**cloud-sync:** 新增云端快照解码辅助工具，并接入主同步流程。',
        ],
      },
      {
        title: '变更',
        items: [
          '**rdp:** 通过共用远程桌面渲染和视口处理改进 RDP 行为。',
          '**settings:** 规范设置标签页处理，并更新终端上下文菜单行为。',
          '**process-manager:** 优化进程显示设置，并移除未使用的进程管理代码。',
          '**ui:** 移除壁纸 Surface 不必要的透明背景样式。',
          '**tuning:** 将发布构建的 panic 行为从 unwind 调整为 abort。',
          '**i18n:** 补充和更新 VNC、录制、连接时长相关的多语言文案。',
        ],
      },
      {
        title: '修复',
        items: [
          '**cloud-sync:** 解码快照时校验源哈希，提升同步数据完整性。',
          '**temporary-session:** 完善临时会话重建流程，并修复临时 SSH 链接上下文菜单操作。',
        ],
      },
      {
        title: '文档',
        items: ['**readme:** 更新 README 中的 RDP 和 VNC 支持说明。', '**docs-site:** 更新会话类型文档，加入 RDP 和 VNC 支持。'],
      },
    ],
  },
  {
    version: '[1.2.2] - 2026-08-12',
    sections: [
      {
        title: '新增',
        items: [
          '**build:** 在发布流程中新增 Windows MSI 打包支持。',
          '**quick-commands:** 新增快捷命令分类拖拽排序。',
          '**rdp:** 增强 RDP 缩放和动态显示行为。',
          '**header:** 新增 macOS 预设编辑菜单项。',
        ],
      },
      {
        title: '变更',
        items: [
          '**terminal-gutter:** 优化 gutter 布局常量，提升可读性。',
          '**i18n:** 新增快捷命令分类创建和自定义排序相关文案。',
        ],
      },
      {
        title: '修复',
        items: [
          '**rdp:** 改进物理按键捕获、锁定键路由、右 Shift 处理，以及认证时 UPN 用户名保留。',
          '**terminal:** 使用 Clear All 时将时间戳重置为终端启动时间。',
          '**release:** 防止远端元数据损坏时发布流程崩溃。',
        ],
      },
      {
        title: '文档',
        items: ['**readme:** 更新 Discord 和微信群链接。'],
      },
    ],
  },
  {
    version: '[1.2.1] - 2026-08-11',
    sections: [
      {
        title: '新增',
        items: [
          '**rdp:** 新增 RDP 会话，支持工作区集成、证书验证、文本剪贴板、适应窗口 / 固定尺寸显示、有限重连以及更好的键盘与 IME 处理。',
          '**ssh:** 新增 SSH Agent 认证与转发，并加入 SSH 连接类型和终端类型选择，便于区分标准服务器和网络设备工作流。',
          '**cloud-sync:** 新增当前云端快照元数据、仅远端变更时自动拉取、冲突恢复动作，以及旧快照对象清理。',
          '**terminal:** 新增大输出 drain、gutter 刷新、时间戳恢复、命令建议快捷键、新建标签 / 视图命令和快速切换器滚动能力。',
          '**recording:** 新增按连接配置录制，并扩展录制捕获、格式化、搜索和测试支持。',
          '**quick-commands:** 新增分类移动、排序、导出、校验优化，以及导入合并时保留分类的行为。',
          '**saved-connections:** 新增展开 / 折叠全部文件夹，并持久化展开状态。',
          '**file-explorer:** 在文件浏览器底部新增已选文件大小与总大小统计。',
          '**macos-menu:** 新增原生 macOS 菜单处理和窗口管理命令。',
          '**serial-send:** 增强串口发送面板的状态管理。',
        ],
      },
      {
        title: '变更',
        items: [
          '**settings:** 共享设置草稿状态处理，让手动同步操作和未保存变更之间的协调更一致。',
          '**layout:** 优化 ResizeHandle 交互样式，并统一 ActivityBar 背景表现。',
          '**claude:** 重构 Claude Code 调用处理，改进 Agent 执行流程。',
          '**i18n:** 补充和刷新 RDP、SSH Agent / 连接类型、云同步恢复、录制、终端建议、文件选择统计、macOS 菜单和快捷命令导出的多语言文案。',
        ],
      },
      {
        title: '修复',
        items: [
          '**rdp:** 改进光标处理、视口缩放和键盘输入行为。',
          '**ssh:** 提高命令注入超时时间，改善慢速 shell 下的可靠性。',
          '**app-context:** 限制屏幕锁只作用于主窗口。',
          '**about:** 改进支持信息处理与展示。',
        ],
      },
      {
        title: '性能',
        items: ['**terminal:** 通过更高效的输出 drain 改善大输出突发时的响应性。'],
      },
      {
        title: '文档',
        items: ['**readme:** 添加 Discord 和微信群徽章，方便社区入口访问。'],
      },
    ],
  },
  {
    version: '[1.2.0] - 2026-08-04',
    sections: [
      {
        title: '新增',
        items: [
          '**gpu-npu-monitor:** 在应用 header / 状态区域新增远程 GPU 和 NPU 总览，通过共享 overview hooks 和紧凑硬件卡片展示活跃 SSH 会话的硬件状态。',
          '**ui:** 在设置中新增 Notes 面板开关，并将 Notes 纳入默认工作区配置。',
          '**file-explorer:** 支持选择多个目录进行上传。',
          '**terminal:** 在 TabBar tooltip 中显示已保存连接的分组路径。',
        ],
      },
      {
        title: '变更',
        items: [
          '**monitoring:** 将远程 GPU / NPU overview 数据整合进现有 GPU 和 Ascend NPU 监控面板。',
          '**i18n:** 为 GPU / NPU 总览、Notes 和已保存连接分组标签补充多语言文案。',
        ],
      },
    ],
  },
  {
    version: '[1.1.19] - 2026-08-04',
    sections: [
      {
        title: '新增',
        items: [
          '**notes:** 新增 Notes 面板和笔记编辑器，支持自动保存、工具栏 / 状态 UI、树形导航、右键菜单、持久化存储、迁移以及同步 / 备份快照。',
          '**asset-monitoring:** 新增资产监控工作区，支持连接分组视图、面包屑导航、表格 / 卡片布局、格式化展示，并集成资源与 GPU 监控。',
          '**sftp:** 增强目录下载与传输处理，包括更完善的 pipeline 行为和 SCP 原始属性处理。',
          '**zmodem:** 新增 Zmodem 传输的本地路径跟踪和在文件管理器中显示的能力。',
          '**quick-commands:** 为快捷命令导入新增保留选项，改进合并流程。',
          '**ai:** 为 Agent 工作流新增原生工具调用模式和错误处理。',
          '**import:** 改进 WindTerm 会话导入，包括主密码提示和合并处理。',
          '**external-open:** 增强 SSH URL 解析与外部连接处理。',
          '**capabilities:** 允许临时目录中的隐藏文件访问能力。',
        ],
      },
      {
        title: '变更',
        items: [
          '**monitoring:** 改进远程统计脚本中的系统信息采集。',
          '**ui:** 简化 Asset 工具栏，移除未使用的标题和数量显示。',
          '**i18n:** 新增并更新 Notes、资产监控、WindTerm 主密码提示、CPU 采样和外部连接内联密码指引相关本地化文案。',
        ],
      },
      {
        title: '修复',
        items: [
          '**monitoring:** 修复 CPU 使用率和百分比格式化中的 `null` 值处理。',
          '**terminal:** 修复搜索栏已打开时再次触发查找后搜索输入框未重新聚焦的问题。',
        ],
      },
      {
        title: '性能',
        items: ['**notes:** 重构 Notes 面板和树组件，提升性能与可用性。'],
      },
      {
        title: '文档',
        items: ['**ssh:** 明确 SSH 连接中内联密码用法的说明。'],
      },
    ],
  },
  {
    version: '[1.1.18] - 2026-07-29',
    sections: [
      {
        title: '新增',
        items: [
          '**external-open:** 新增外部连接打开处理、URL 解析、已保存连接匹配和确认对话框，可从 NyaTerm 外部打开连接链接。',
          '**terminal:** 新增 `TerminalFitScheduler` 与相关测试，增强搜索生命周期 hooks，支持终端外部文件拖放，并在重连和可见性变化后更可靠地刷新终端布局与输出。',
          '**session:** 新增会话附加支持和休眠日志。',
          '**sftp:** 新增带 controller 支持的远程文件复制，并在远程文件写入时恢复权限。',
          '**file-explorer:** 新增可编辑路径按钮、搜索入口、活跃会话目标和文件窗口目标处理增强。',
          '**header:** 新增 header 状态可见性控制、确认对话框，并刷新 header 操作和图标。',
          '**security-auth:** 新增凭据、私钥和 OTP 管理对话框，并增强密钥管理 UI 与后端处理。',
          '**network:** 新增 Proxy 和 Tunnel 管理页面。',
          '**quick-commands:** 新增命令预览、复制能力和分类选择状态管理。',
          '**ai-settings:** 新增 AI 模型刷新，并改进模型发现。',
          '**updater:** 新增 Windows 便携版自动更新支持。',
        ],
      },
      {
        title: '变更',
        items: [
          '**sftp:** 模块化 SFTP 后端，并改进上传取消处理。',
          '**importer:** 统一各导入器的 secret 加密逻辑。',
          '**updater:** 将便携版更新统一到 Cloudflare R2。',
          '**docker:** 更新 Docker 命令路径。',
          '**settings:** 改进关闭处理和未保存变更确认。',
          '**i18n:** 更新凭据、SSH 密钥、代理 / 隧道、终端显示、header 状态和外部连接相关本地化文案。',
        ],
      },
      {
        title: '修复',
        items: [
          '**terminal:** 改进终端快照序列化与重连处理，在 Enter 和命令建议执行时刷新时间戳，并改进同步输入的重连 / 会话处理。',
          '**sync:** 使用会话输入 peer ID 选择同步输入目标。',
          '**window:** 避免子窗口加载闪烁，并改进子窗口日志和冲突警告。',
          '**sftp:** 使用 raw path 匹配改进目录删除逻辑。',
          '**file-explorer:** 改进路径栏 resize 处理。',
          '**ssh:** 将命令注入超时时间从 5 秒增加到 30 秒。',
          '**redaction:** 新增敏感数据 marker value 脱敏。',
          '**ui:** 改进 SSH 表单和新建会话表单的响应式 class。',
        ],
      },
      {
        title: 'CI',
        items: [
          '**updater:** 为 `latest.json` 新增 signed portable target 校验。',
          '**gitee:** 改进 release workflow 中的 Python 依赖安装。',
        ],
      },
      {
        title: '文档',
        items: ['**readme:** 更新中文 README 的 badge 链接和格式。'],
      },
    ],
  },
  {
    version: '[1.1.17] - 2026-07-22',
    sections: [
      {
        title: '修复',
        items: [
          '**csp:** 在内容安全策略中允许 `blob:` 图片 URL，使生成或预览的图片资源可以正确渲染。',
          '**terminal:** 改进 XTerminal 组件的休眠逻辑。',
          '**zmodem:** 改进 Zmodem 上传过程中的冲突处理。',
        ],
      },
      {
        title: '文档',
        items: ['**readme:** 在 README 文件中新增 macOS 安装说明。', '**i18n:** 优化 Zmodem 冲突处理提示文案，使表达更清晰。'],
      },
    ],
  },
  {
    version: '[1.1.16] - 2026-07-21',
    sections: [
      {
        title: '新增',
        items: [
          '**temporary-link:** 整合临时连接处理流程，并增强一次性会话的协议支持。',
          '**file-explorer:** 新增文件复制、后端文件操作支持、raw path token 处理、目录导航缓存和文件预览流程。',
          '**quick-commands:** 新增快捷命令脚本编辑器，并支持复制快捷命令。',
          '**terminal:** 新增可配置时间戳格式、SSH keep-alive 模式、断开状态输出刷新、命令建议键盘导航优化和断开标签指示。',
          '**settings:** 新增应用语言保存能力，使语言切换可通过后端持久化。',
          '**icons:** 新增 Kubernetes、macOS、Android 图标资源，并改进会话启动后的远程系统图标自动识别。',
          '**zmodem:** 新增上传时保留时间戳选项，并为未验证目标提供更安全的冲突提示。',
          '**saved-connections:** 新增键盘导航、连接元素注册和移动到分组操作。',
          '**ai:** 新增 Codex 与 Claude Code 集成、运行模式选择、模型过滤、自定义 provider、命令目标上下文和动态工具调用跟踪。',
          '**clipboard:** 改进 Windows 剪贴板写入和 fallback 机制。',
          '**cloud-sync:** 新增连接成功处理，并改进同步 / 备份模式的 portable snapshot 处理。',
          '**theme:** 新增主题设计器对话框并增强主题管理。',
          '**i18n:** 新增繁体中文本地化和语言处理。',
          '**session-targets:** 增强发送命令目标结构和兼容性。',
          '**tunnel:** 新增隧道运行态管理和连接跟踪。',
          '**tray:** 新增 macOS 托盘图标支持。',
          '**monitoring:** 新增 Ascend NPU 监控面板和设置。',
          '**header:** 新增远程状态支持和 header 状态模式。',
        ],
      },
      {
        title: '变更',
        items: [
          '**header:** 简化会话图标处理。',
          '**ssh:** 重命名并更新 suppressed output 处理辅助函数。',
          '**terminal:** 更新本地 PTY 环境配置，提升跨平台兼容性。',
          '**sftp:** 规范化远程目录路径并改进路径拼接。',
          '**settings:** 将 macOS IME 兼容设置重命名为通用 IME 兼容设置。',
          '**ai:** 移除 AgentStepView 中冗余的 Markdown 渲染。',
        ],
      },
      {
        title: '修复',
        items: [
          '**clipboard:** 保持 arboard 剪贴板存活并启用 Wayland 支持。',
          '**context:** 重构标签会话的启动恢复逻辑。',
          '**panel:** 改进已保存连接项和分组节点的 pointer-event 处理。',
          '**cloud-sync:** 为所有 provider 初始化 OpenDAL transport，并升级 OpenDAL 到 0.58。',
          '**build:** russh crypto backend 改用 `ring`，替代 `aws-lc-rs`。',
          '**terminal:** 修复 GBK 编码支持和 Linux Fcitx IME 重复输入问题。',
          '**icons:** 改进远程系统发行版匹配。',
        ],
      },
      {
        title: '性能',
        items: ['**terminal:** 改进终端输出处理、性能模式行为和 WebGL 资源管理。'],
      },
      {
        title: '文档',
        items: ['**i18n:** 新增 SSH keep-alive 描述，并更新新工作流本地化文案。', '**images:** 更新明暗主题产品图片。'],
      },
    ],
  },
  {
    version: '[1.1.15] - 2026-07-14',
    sections: [
      {
        title: '修复',
        items: ['**app:** 移除 macOS 特定的标签关闭处理，并简化 effect 依赖。'],
      },
      {
        title: 'CI',
        items: ['**gitee:** 优化 Gitee release workflow 的触发和 tag 解析。'],
      },
    ],
  },
  {
    version: '[1.1.14] - 2026-07-14',
    sections: [
      {
        title: '新增',
        items: [
          '**telnet:** 新增自动登录和启动命令支持。',
          '**tabs:** 新增标签锁定，避免意外关闭。',
          '**terminal:** 新增串口远程颜色 OSC 防护、终端缩放设置、终端主题与布局增强，以及标准化输入行为。',
          '**remote-stats:** 集成远程系统状态和连接图标自动识别。',
          '**ai:** 为模型控件新增 reasoning effort 选择。',
          '**connections:** 新增批量删除支持，并恢复上次打开连接和分组展开状态。',
          '**webview:** 支持阻止 webview 内容拦截保留快捷键。',
          '**fonts:** 新增 JetBrainsMono Nerd Font Mono 并更新字体默认值。',
          '**xterm:** 为 NyaTerm 隔离 xterm WebGL texture atlas。',
          '**build:** 新增解锁救援脚本和 xterm WebGL post-install patch。',
          '**docker:** 新增 Docker sudo 密码处理。',
          '**security-auth:** 新增已保存密码认证、凭据解锁动作、凭据重排、Tab 键凭据选择，以及感知活动会话的 OTP 管理。',
          '**appearance:** 新增窗口透明度控制并改进字体处理。',
          '**i18n:** 新增韩语本地化。',
          '**sftp:** 新增 SFTP 设置和远程文件浏览器。',
          '**header:** 新增取消分屏动作。',
        ],
      },
      {
        title: '修复',
        items: [
          '**terminal:** 改进终端右键菜单剪贴板行为，避免 macOS trackpad 产生 stale selection，并支持 Ctrl printable 终端按键。',
          '**stats:** 确保磁盘数据文件被正确创建和填充。',
          '**panel:** 修复多单元进度计算。',
          '**network:** 将连接选项限制为仅 SSH 连接。',
          '**ssh:** 确保 post-login timer 只在 normal 阶段启动。',
        ],
      },
      {
        title: '性能',
        items: ['**terminal:** 改进输出处理和流控。', '**zmodem:** 新增上传和下载 drain 机制。'],
      },
      {
        title: '文档',
        items: [
          '**readme:** 更新贡献者图片链接。',
          '**installation:** 新增 Windows portable 直链。',
          '**images:** 为文档站新增监控和导入图片。',
          '**changelog:** 更新 changelog 到 `1.1.13`。',
        ],
      },
    ],
  },
  {
    version: '[1.1.13] - 2026-07-06',
    sections: [
      {
        title: '新增',
        items: [
          '**gpu:** 为 SSH 主机新增 GPU 监控面板，展示驱动 / CUDA 版本、每张 GPU 的使用率、显存、温度、功耗、风扇，以及可搜索的 GPU 进程占用列表，并提供 `显示 GPU 监控` 开关和可配置轮询间隔。',
          '**docker:** 为 SSH 主机新增 Docker 管理面板，涵盖容器、镜像、数据卷、网络和 Compose 项目，支持容器详情、日志、进入 / exec、生命周期操作，以及删除、kill、compose down、`system prune` 等需确认的破坏性操作。',
          '**process:** 为 SSH 主机新增进程管理面板，提供实时、可排序、可搜索的进程列表、进程详情、`renice` 以及信号发送（TERM/HUP/STOP/CONT 及需确认的 KILL）。',
          '**app:** 跟踪存活会话状态，使监控面板只绑定到真正处于活动状态的 SSH 会话；并对活动栏面板可见性做门控，避免自动打开已关闭的面板。',
          '**ssh:** 新增按连接维度的 SSH 算法偏好，提供兼容 / 安全 / 自定义三种模式，支持可重新排序的密钥交换、加密、MAC、主机密钥列表，并标注现代 / 旧版 / 不安全风险等级。',
          '**connections:** 新增临时 SSH 链接对话框，可从粘贴的 `ssh://` URL 或 `ssh` 命令字符串打开一次性会话，而不保存连接。',
          '**terminal:** 新增会话输入同步能力，通过命名同步组把按键镜像到组内其他会话，并在发送命令面板中提供当前会话 / 全部会话 / 指定分组的目标选择。',
          '**file-transfer:** 让 Zmodem（rz/sz）传输以合适的控制方式显示在传输列表中并带实时进度。',
          '**terminal:** 新增 Unicode grapheme 支持，正确渲染 emoji、组合字符和 ZWJ 序列的显示与光标宽度。',
          '**terminal:** 新增清空输入动作（Ctrl/Cmd+L），可从右键菜单和快捷键触发。',
          '**terminal:** 新增关闭全部会话前的确认对话框。',
          '**ai:** 为 AI 助手新增后台执行设置。',
          '**app-lock:** 为锁屏新增窗口关闭确认和控制。',
          '**window:** 增强主窗口状态管理和定位。',
          '**importer:** 从 WindTerm 会话导入中提取用户和主机。',
        ],
      },
      {
        title: '变更',
        items: [
          '**terminal:** 扩展内置关键词高亮预设，覆盖更多错误和成功短语。',
          '**ai:** 改进 Deepseek provider 的模型名处理。',
          '**serial:** 优化串口会话表单布局的响应式表现。',
        ],
      },
      {
        title: '修复',
        items: [
          '**russh:** 改进 name-list 解析，处理尾随逗号并拒绝非法条目。',
          '**ssh:** 改进 SSH handler 中的 X11 通道处理。',
          '**ssh:** 抑制 Windows 上运行本地系统 shell 命令时闪现的控制台窗口。',
          '**hooks:** 为强制凭据加载新增 reload 处理。',
        ],
      },
      {
        title: '性能',
        items: [
          '**sftp:** 新增通道并发限制，并对临时性 SFTP 通道打开失败进行带退避的自动重试。',
          '**watcher:** 使用内容指纹，使自动上传仅在内容真正变化时触发，而非编辑器仅改动元数据的保存。',
          '**hooks:** 为凭据加载新增强制重新加载选项。',
        ],
      },
    ],
  },
  {
    version: '[1.1.12] - 2026-06-30',
    sections: [
      {
        title: '新增',
        items: ['**connections:** 增强跳板机配置，改进 ProxyJump 风格链路的展示与校验。'],
      },
      {
        title: '修复',
        items: ['**connections:** 阻止跳板机关系形成环路，避免已保存连接递归引用。'],
      },
      {
        title: '文档',
        items: ['**i18n:** 为更新后的跳板机工作流补充本地化字符串。'],
      },
    ],
  },
  {
    version: '[1.1.11] - 2026-06-30',
    sections: [
      {
        title: '新增',
        items: [
          '**header:** 新增命令面板入口，便于快速查找应用动作和会话工作流。',
          '**ai:** 为 Agent 交互引入命令执行与最终回答工具，同时保留审批门槛。',
          '**app-lock:** 实现应用锁定状态管理，并支持空闲自动锁定。',
          '**terminal-layout:** 恢复终端窗口布局状态，并加入持久工作区相关设置。',
          '**appearance:** 新增终端普通文本与粗体文本的字重选项。',
          '**app:** 新增最小化到托盘和隐藏主窗口行为，适合后台驻留工作流。',
        ],
      },
      {
        title: '变更',
        items: [
          '**window-management:** 改进子窗口状态持久化和模态焦点处理。',
          '**themes:** 刷新终端与应用主题颜色，包括高对比度变体。',
        ],
      },
      {
        title: '修复',
        items: [
          '**saved-connections:** 改进连接交互和拖拽行为中的 pointer event 处理。',
          '**terminal:** 改进嵌套工作区根节点中的终端缩放处理。',
        ],
      },
    ],
  },
  {
    version: '[1.1.10] - 2026-06-25',
    sections: [
      {
        title: '新增',
        items: [
          '**ssh-auth:** 新增专用 SSH 认证请求对话框，用于交互式登录流程。',
          '**proxy:** 在代理配置中新增 ProxyCommand 支持，并兼容 OpenSSH 风格占位符。',
          '**transfer-dialog:** 为上传和下载新增重复目标处理选项。',
          '**terminal:** 改进终端搜索，支持持久查询状态、结果跳转和更清晰反馈。',
          '**cloud-sync:** 新增更详细的云同步状态更新和清理超时处理。',
        ],
      },
      {
        title: '变更',
        items: [
          '**ai:** 精简 AI 模型客户端的服务目标配置。',
          '**cloud-sync:** 调整云同步历史和日志处理，便于诊断。',
        ],
      },
      {
        title: '修复',
        items: [
          '**terminal:** 改进搜索焦点处理和选区清理。',
          '**settings:** 修复新建会话页面中的已保存连接排序。',
        ],
      },
      {
        title: '性能',
        items: ['**serial:** 精简串口会话管理并改进超时处理。'],
      },
    ],
  },
  {
    version: '[1.1.9] - 2026-06-21',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal:** 为复制会话和多路复用会话新增启动命令。',
          '**terminal:** 在终端设置中新增图片路径粘贴行为。',
          '**terminal:** 新增工作区 padding 设置，用于调整终端布局间距。',
          '**interaction:** 新增 macOS IME 兼容性设置。',
          '**telnet:** 为 Telnet 会话新增本地行编辑。',
          '**terminal:** 增强 Windows Terminal 支持和 shell 选择。',
        ],
      },
      {
        title: '变更',
        items: [
          '**terminal:** 在交互式程序中抑制命令建议，避免内联建议干扰输入。',
          '**sync-backup:** 精简同步备份历史和云同步管理器行为。',
        ],
      },
      {
        title: '修复',
        items: [
          '**tabbar:** 阻止标签交互中的 pointer event 传播问题。',
          '**terminal:** 改进重连内容管理和连接错误处理。',
        ],
      },
    ],
  },
  {
    version: '[1.1.8] - 2026-06-21',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal:** 新增标签页分屏和拖拽停靠支持。',
          '**ssh:** 为 SSH 连接新增 X11 转发支持。',
          '**russh-sftp:** 引入新的 SFTP 子系统，提供客户端和服务端支持。',
          '**file-explorer:** 新增远程文件编辑流程和专用远程文件处理对话框。',
          '**sftp:** 新增 OpenSSH 兼容的符号链接支持。',
          '**file-transfer:** 在传输界面显示传输速度。',
          '**cloud-sync:** 增强云同步和 GitHub Gist 的启动检查与错误处理。',
          '**saved-connections:** 新增连接复制快捷键，并改进已选连接的拖拽行为。',
        ],
      },
      {
        title: '修复',
        items: [
          '**scp:** 为远端命令增加 GNU `-printf` 和 `-c` 支持检查。',
          '**file-explorer:** 改进目录加载、下载处理和错误反馈。',
        ],
      },
      {
        title: '性能',
        items: [
          '**russh-sftp:** 提升 SFTP 上传吞吐和传输跟踪。',
          '**sftp:** 对已知大小的文件使用异步下载。',
        ],
      },
      {
        title: '文档',
        items: [
          '**ssh:** 在 SSH 连接指南中记录 X11 转发。',
          '**cloud-sync:** 更新云同步术语和 GitHub Gist 授权说明。',
        ],
      },
    ],
  },
  {
    version: '[1.1.7] - 2026-06-15',
    sections: [
      {
        title: '新增',
        items: [
          '**panel:** 新增多面板打开行为，可在不丢失上下文的情况下打开多个工具。',
          '**terminal:** 新增后端输出暂停/恢复控制。',
          '**session:** 新增会话快速切换器对话框，便于在活动会话和已保存连接间快速导航。',
          '**icons:** 新增服务器图标并改进已保存连接的图标解析。',
          '**network:** 为代理和隧道新增分组管理。',
          '**sync:** 新增 v3 快照解码和 payload hash 计算。',
        ],
      },
      {
        title: '变更',
        items: [
          '**file-explorer:** 定义共享 `FileProperties` 接口并清理属性对话框。',
          '**terminal:** 改进标签提示、标签管理和滚动行为。',
        ],
      },
      {
        title: '修复',
        items: [
          '**command-history:** 修正命令建议应用行为。',
          '**app:** 改进 Windows 上权限错误时的文件夹打开逻辑。',
          '**file-explorer:** 修复远程文件下载后打开目标目录失败的问题。',
        ],
      },
    ],
  },
  {
    version: '[1.1.6] - 2026-06-12',
    sections: [
      {
        title: '新增',
        items: [
          '**session:** 新增可搜索的会话快速切换器，可通过键盘或鼠标在已保存连接和本地会话之间跳转，并提供新建 SSH 会话的快捷入口。',
          '**recording:** 新增自动开始录制选项，会话打开时自动开始录制。',
          '**terminal:** 序列化终端文本，使重连的会话能够恢复之前的屏幕输出。',
          '**terminal:** 支持关闭已断开连接的窗格。',
          '**terminal:** 新增删除单条命令历史记录的功能。',
          '**file-explorer:** 增强属性对话框中的文件属性管理。',
          '**clipboard:** 实现带超时的异步剪贴板读取。',
          '**action-links:** 在压缩包动作链接匹配器中支持 RAR 文件。',
          '**ai:** 增强模型管理，支持手动添加模型和凭据分组。',
          '**transfer:** 实现后台传输并发数的动态调整。',
          '**stats:** 增强 `SYSINFO_SCRIPT`，改进系统信息采集。',
        ],
      },
      {
        title: '变更',
        items: [
          '**quick-commands:** 引入排序模式并更新视图模式默认值。',
          '**sftp:** 增强 SFTP 后端的目录和符号链接处理。',
          '**password-management:** 增强密码面板的解锁逻辑和底栏可见性。',
        ],
      },
      {
        title: '修复',
        items: [
          '**connection:** 改进密码管理的编辑流程。',
          '**macos:** 支持在 macOS 上拖拽重新排序连接。',
          '**file-explorer:** 重置水平滚动位置并改进文件项交互。',
          '**terminal:** 优化终端上下文菜单中的右键粘贴功能。',
          '**ssh:** 重组导入并调整首选算法。',
        ],
      },
      {
        title: '性能',
        items: ['在加密前压缩便携快照。'],
      },
      {
        title: '文档',
        items: ['**README:** 新增 NyaTerm 的 Arch Linux 安装说明。'],
      },
    ],
  },
  {
    version: '[1.1.5] - 2026-06-09',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal:** 新增多路复用 SSH 会话支持，单个连接即可承载多个终端。',
          '**ai:** 引入独立的 AI Assistant 面板及相关组件。',
          '**ai:** 新增命令风险等级，用于控制命令执行。',
          '**ai:** 新增请求 User-Agent 配置及默认值处理。',
          '**cloud-sync:** 新增 Gitee 代码片段云同步提供商。',
          '**terminal:** 增强多行粘贴处理，加入专用对话框和输入状态管理。',
          '**terminal:** 实现终端输入中的本地退格处理。',
          '**terminal:** 在终端显示中支持时间戳毫秒。',
          '**quick-commands:** 新增紧凑视图模式和视图模式切换，并加入命令删除确认对话框。',
          '**file-explorer:** 为目录实现收藏夹功能。',
          '**key-management:** 在密钥编辑器中支持证书文件处理。',
          '**search:** 改进搜索引擎管理，支持动态生成键值。',
          '**local-terminal:** 新增 shell 参数支持和 shell 路径的文件选择。',
          '**pty:** 新增用于 shell 集成的本地启动脚本，并在启动期间抑制输出。',
          '**themes:** 新增 Nya High Contrast 主题并刷新调色板。',
        ],
      },
      {
        title: '变更',
        items: [
          '**app:** 根据当前活动标签页更新窗口标题。',
          '**window:** 为子窗口实现 owner window label 处理，并扩展窗口能力匹配模式。',
          '**ssh:** 增强提示注入处理和 OSC 处理。',
          '**ssh:** 增强键盘交互认证中的密码提示处理。',
          '**ai:** 通过基于语言环境的提示词选择，使 AI 输出跟随应用语言。',
          '**credential-management:** 改进正则校验和提示处理。',
        ],
      },
      {
        title: '修复',
        items: [
          '**deps:** 将 xterm 依赖更新到 beta 版本。',
          '**macos:** 规范化打包后的 macOS PTY 环境。',
          '**terminal:** 初始化 XTerminal 中的断开和重连状态。',
        ],
      },
    ],
  },
  {
    version: '[1.1.4] - 2026-06-03',
    sections: [
      {
        title: '新增',
        items: [
          '**ssh:** 在 SSH 表单和新建会话页中实现登录后命令支持。',
          '**ssh:** 为 SSH 表单中的密码字段新增必填标识。',
          '**saved-connections:** 增强搜索功能，扩展分组管理能力。',
          '**serial:** 增强串口会话处理，改进错误日志并支持 Zmodem 检测。',
          '**panel:** 增强发送命令面板，加入十六进制数据处理并重构状态管理。',
          '**recording:** 为录制新增包含时间戳的选项。',
          '**terminal:** 增强终端行号槽，支持动态单元格尺寸和布局调整，并改进整体输入体验。',
        ],
      },
      {
        title: '变更',
        items: [
          '**zmodem:** 改进 Zmodem 事件处理和检测逻辑。',
          '**terminal:** 统一命令建议和凭据建议的光标位置处理。',
          '**terminal:** 改进多行粘贴对话框的焦点处理。',
        ],
      },
      {
        title: '修复',
        items: ['**header:** 关闭窗口时遵循最小化到托盘的设置。'],
      },
    ],
  },
  {
    version: '[1.1.3] - 2026-06-02',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal:** 实现多行粘贴对话框并增强粘贴处理。',
          '**terminal:** 增强输入处理，加入逻辑行快照和选区范围跟踪。',
          '**terminal:** 实现凭据提示检测和输入处理。',
          '**panel:** 增强发送命令面板的 shell 命令功能。',
          '**recording:** 实现会话录制，包括开始/停止、保存转录文本和内存限制设置。',
          '**file-explorer:** 在路径栏中新增按会话维度的目录历史。',
          '**import:** 在导入对话框中支持从 NyaTerm JSON 格式导入会话。',
          '**security:** 增强主密码管理，改进校验和界面。',
          '**quick-commands:** 新增用于删除和重命名快捷命令分类的对话框。',
          '**docs-site:** 新增离线本地搜索。',
        ],
      },
      {
        title: '变更',
        items: [
          '**cloud-sync:** 实现云同步操作超时，并增强存储操作的错误处理。',
          '**cloud-sync:** 新增 WebDAV Digest 认证支持。',
          '**cloud-sync:** 为云同步操作实现自动重试机制。',
          '**backup:** 将备份文件扩展名从 `.dgfy` 更新为 `.nya`。',
          '**header:** 将菜单标签从 `New SSH Connection` 重命名为 `New Session`。',
        ],
      },
      {
        title: '修复',
        items: [
          '**settings:** 通过管理保存状态，确保保存后正确关闭设置窗口。',
          '**app:** 改进 `safeRecordingName` 的规范化逻辑，允许更广泛的合法字符。',
          '**docs-site:** 改进导航栏响应式表现和浮动搜索框。',
        ],
      },
    ],
  },
  {
    version: '[1.1.2] - 2026-05-30',
    sections: [
      {
        title: '新增',
        items: [
          '**window-state:** 实现主窗口状态管理，持久化窗口尺寸和位置。',
          '**quick-commands:** 支持导入 Xshell 快捷按钮。',
        ],
      },
      {
        title: '变更',
        items: ['**app:** 集中处理子窗口的尺寸和位置。'],
      },
      {
        title: '文档',
        items: ['在 README 中新增贡献者章节和 star 历史图表。'],
      },
    ],
  },
  {
    version: '[1.1.1] - 2026-05-29',
    sections: [
      {
        title: '修复',
        items: ['移除未使用的子窗口预加载逻辑和背景色处理。'],
      },
    ],
  },
  {
    version: '[1.1.0] - 2026-05-29',
    sections: [
      {
        title: '新增',
        items: [
          '**serial:** 实现波特率选择器，用于选择和校验串口波特率。',
          '**file-transfer:** 新增下载功能，并通过入队下载增强传输管理。',
          '**file-transfer:** 新增 `queued` 状态并改进传输界面交互。',
          '**file-explorer:** 根据当前活动连接自动同步当前工作目录。',
          '**transfer:** 实现并发数限制，并将下载/上传线程设置重命名为并发任务数。',
          '**errors:** 在新建会话页中新增认证失败提示并增强校验。',
        ],
      },
      {
        title: '变更',
        items: [
          '**file-explorer:** 新增刷新后的 FileExplorer 组件及文件操作对话框，包括路径栏和工具栏。',
          '**ui:** 使用 `ActionButton` 和 `ActionFooter` 统一对话框底栏，并将 Toaster 移入主布局。',
        ],
      },
      {
        title: '修复',
        items: [
          '**modal:** 防止子窗口关闭后模态遮罩层残留。',
          '**child-windows:** 通过窗口预加载减少启动闪烁。',
          '**build-release:** 修正 Ubuntu ARM 安装的包名。',
        ],
      },
      {
        title: '性能',
        items: ['**sftp:** 增强 SFTP 后端，支持可配置的客户端设置和性能日志。'],
      },
    ],
  },
  {
    version: '[1.0.9] - 2026-05-27',
    sections: [
      {
        title: '新增',
        items: [
          '**ssh:** 在 SSH 表单中引入 `none` 认证模式，丰富连接选项。',
          '**runtime:** 实现便携模式支持，并新增 Windows 便携版 zip 发布。',
          '**quick-commands:** 新增快捷命令导入对话框，支持 WindTerm 和 NyaTerm 格式。',
          '**terminal:** 新增断开会话功能。',
          '**file-explorer:** 为文件项实现内联重命名。',
          '**file-transfer:** 增强进度跟踪并新增清空全部操作。',
        ],
      },
      {
        title: '变更',
        items: [
          '**file-transfer:** 更新传输行状态图标并简化状态处理。',
          '**dialogs:** 改进对话框的响应式宽度处理和更新对话框的 Markdown 渲染。',
        ],
      },
      {
        title: '修复',
        items: [
          '**session-input:** 修复由换行转换导致的快捷命令问题。',
          '**telnet:** 集成录制管理器以处理会话输入/输出。',
          '**shortcuts:** 校验并支持自定义标签切换快捷键。',
          '**saved-connections:** 更新空状态判断，同时检查已保存连接和分组。',
          '**workflows:** 将发布工作流中的 Ubuntu 版本从 24.04 更新为 22.04。',
        ],
      },
    ],
  },
  {
    version: '[1.0.8] - 2026-05-24',
    sections: [
      {
        title: '新增',
        items: [
          '**quick-commands:** 为快捷命令实现排序和使用次数跟踪。',
          '**readme:** 新增 GitHub 下载量徽章。',
        ],
      },
      {
        title: '变更',
        items: [
          '**settings:** 更新应用设置管理并引入 UI 设置保存。',
          '**storage:** 重构存储模块并迁移到类型化设置文档。',
          '**security:** 更新安全标签页中的主密码处理。',
          '**scrollbar:** 隐藏滚动条按钮并改进滚动条外观。',
        ],
      },
      {
        title: '修复',
        items: ['**errors:** 改进新建会话页和快捷命令页的错误处理。'],
      },
    ],
  },
  {
    version: '[1.0.7] - 2026-05-21',
    sections: [
      {
        title: '变更',
        items: [
          '**appearance:** 改进字体选择界面，并在系统字体发现过程中显示 `Loading system fonts...` 加载状态。',
          '**otp:** 优化 OTP 输入槽位布局和 OTP 代码面板在窄屏下的响应式表现。',
          '**profiles:** 为调试和发布流程新增多个 Cargo 构建 profile。',
        ],
      },
      {
        title: '修复',
        items: ['**csp:** 修正 CSP 中的资源协议处理，确保背景图等本地资源可以稳定加载。'],
      },
      {
        title: '性能',
        items: ['**appearance:** 改为异步加载系统字体，保持外观设置页的响应速度。'],
      },
    ],
  },
  {
    version: '[1.0.6] - 2026-05-21',
    sections: [
      {
        title: '新增',
        items: [
          '**appearance:** 为主窗口新增 `Background Image` 自定义能力，支持 `Image Sizing`、`Image Opacity` 和 `Background Content Opacity` 控制。',
          '**sessions:** 为 Telnet 和串口会话新增 `Backspace Mode` 选项，可在 `Ctrl+H (BS)` 与 `DEL (0x7F)` 之间切换。',
        ],
      },
      {
        title: '变更',
        items: ['**resource-monitor:** 刷新资源监控面板的卡片样式和视觉层级。'],
      },
      {
        title: '修复',
        items: [
          '**shortcuts:** 修复 Shift 修饰键输入被误判为应用快捷键的问题，恢复 `Shift+C`、`Shift+V`、`Shift+X` 等大写输入。',
        ],
      },
    ],
  },
  {
    version: '[1.0.5] - 2026-05-19',
    sections: [
      {
        title: '新增',
        items: [
          '**ai:** 捕获 AI 命令执行事件，并在 Agent 工作流中渲染终端内联输出。',
          '**ai:** 新增 `Terminal Output Lines` 设置，用于控制 AI 执行命令时显示的内联输出行数。',
          '**terminal:** 为终端会话表单新增 AI Execution Profile 选择。',
          '**window:** 改进主窗口模态管理，让子窗口触发时工作区的模态状态更清晰。',
        ],
      },
    ],
  },
  {
    version: '[1.0.4] - 2026-05-19',
    sections: [
      {
        title: '新增',
        items: [
          '**cloud_sync:** 实现云同步功能，加入加密支持与日志记录。',
          '**ai:** 引入 AgentApprovalManager 并重构 AI 命令处理逻辑。',
          '**session-management:** 增强会话管理，新增 initialGroupId 支持。',
        ],
      },
      {
        title: '变更',
        items: [
          '**ai:** 移除 AI 组件及配置中的风险评估功能和 AiRiskLevel。',
          '**i18n:** 更新本地化文件，补充 agent 命令执行与审批提示。',
          '**window:** 改进模态窗口的置顶逻辑。',
          '**file-explorer:** 优化文件拖放处理逻辑。',
          '**sftp:** 移除 SSH 命名空间并引入新的后端结构以整合 SFTP 处理。',
          '**ssh-form:** 更新 SshForm 和 NewSessionPage，改进密码处理与连接管理。',
          '**cleanup:** 移除 AiTab 和 XTerminal 中未使用的组件和函数。',
        ],
      },
    ],
  },
  {
    version: '[1.0.3] - 2026-05-18',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal:** 在终端管理中实现标签页移动和解除分屏功能。',
          '**zmodem:** 实现 Zmodem 文件传输命令、检测和事件处理。',
          '**shortcuts:** 实现自定义键盘快捷键及设置管理。',
          '**window:** 增强模态子窗口处理。',
          '**ai:** 增强 AI Assistant 面板，改进空状态显示。',
        ],
      },
      {
        title: '变更',
        items: [
          '**appearance:** 重构字体管理并更新默认字体设置。',
          '**i18n:** 在本地化文件中新增 Zmodem 传输信息、终端字体设置、快捷键及 AI 设置指南。',
          '**accessibility:** 为多个对话框新增 DialogDescription 组件以提升无障碍体验。',
          '**theme:** 更新 githubDark 主题中的颜色值以提升一致性。',
        ],
      },
    ],
  },
  {
    version: '[1.0.2] - 2026-05-17',
    sections: [
      {
        title: '新增',
        items: [
          '**credentials:** 实现凭据管理功能，支持对话框及终端自动填充。',
          '**security:** 增强密码管理，支持密钥解锁和密码显示功能。',
          '**terminal:** 在 TerminalTab 组件中新增关键词高亮设置及功能。',
          '**clipboard:** 新增 CopyButton 组件用于剪贴板操作。',
          '**tabbar:** 增强 TabBar 组件，支持动态标签页可见性管理和溢出处理。',
          '**templates:** 新增缺陷报告和功能请求的 Issue 模板。',
        ],
      },
      {
        title: '变更',
        items: [
          '**theme:** 更新主题颜色及 CSS 变量以提升一致性。',
          '**i18n:** 新增密码管理、搜索功能及隐藏会话的本地化条目。',
          '**ui:** 在会话表单中新增必填项指示，改进标签样式。',
          '**terminal:** 增强 ActionLinksAddon 和 KeywordHighlighter，改进计时器管理与刷新逻辑。',
          '**docs:** 在文档中补充 AI 助手和安全功能相关内容。',
        ],
      },
    ],
  },
  {
    version: '[1.0.1] - 2026-05-16',
    sections: [
      {
        title: '新增',
        items: [
          '**highlighting:** 在 keywordHighlightPresets 中新增命令行提示符高亮支持。',
          '**app:** 在 Tauri 应用中集成单例运行支持。',
          '**docs:** 在 Docusaurus 配置中新增 Umami 分析插件。',
          '**ci:** 新增用于 R2 资产发布的 GitHub Actions 工作流。',
        ],
      },
      {
        title: '变更',
        items: [
          '**file-explorer:** 清理 FileExplorer 组件，移除未使用代码并优化状态管理。',
          '**upload:** 简化 AutoUploadPage 中的文件上传处理逻辑。',
          '**highlighting:** 更新深色和浅色规则板中的操作符颜色，以及操作符的正则匹配模式。',
          '**docs:** 更新更新日志，补充新版本与增强内容。',
          '**ci:** 更新 Docusaurus 依赖，移除可选的 Umami 配置。',
        ],
      },
    ],
  },

  {
    version: '[1.0.0] - 2026-05-06',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal-ai:** 在 XTerminal 中新增 AI 输出捕获，并支持基于标记的命令执行输出捕获。',
          '**connections:** 增强连接管理，加入最近连接跟踪，并补充对应本地化文案。',
          '**downloads:** 增强下载平台管理，支持架构识别和动态获取发布资产。',
          '**release:** 新增 Cloudflare R2 发布流程和用于发布资产上传的 GitHub Actions 工作流。',
          '**branding:** 更新 NyaTerm logo SVG，使用新的渐变和眼部镂空遮罩。',
        ],
      },
      {
        title: '变更',
        items: [
          '**shell:** 移除 ShellKind 及相关逻辑中的 PowerShell 支持。',
          '**branding:** 将文档和代码库中的 Dragonfly 引用替换为 NyaTerm。',
          '**updater:** 更新 Tauri updater endpoint，以改进版本获取流程。',
          '**deps:** 新增 strip-ansi-escapes 和 vte 依赖，以提升终端输出处理的可靠性。',
          '**ci:** 清理过期的调试发布工作流。',
        ],
      },
      {
        title: '修复',
        items: [
          '**workflow:** 在发布工作流中下载 GitHub Release 资产。',
          '**workflow:** 为 build-release 工作流新增 TAG 环境变量。',
        ],
      },
      {
        title: '文档',
        items: ['**homepage:** 更新首页在深色和浅色主题下的图片。'],
      },
    ],
  },
  {
    version: '[0.9.0] - 2026-04-30',
    sections: [
      {
        title: '新增',
        items: [
          '**ai-assistant:** 将 AI Assistant 集成到应用中，支持终端和文件浏览器动作、会话历史搜索、会话分组、复制选择内容以及删除会话。',
          '**agent:** 新增 agent 模式，支持命令执行、最大步骤和超时设置、命令风险评估、chmod/chown 高风险模式以及带语法高亮的步骤视图。',
          '**ai-chat:** 增强 AI 聊天流处理，加入会话管理、清理、推理内容、Markdown 支持、结构化输出解析和更完善的日志。',
          '**storage:** 实现基于 redb 的用户数据存储，并支持 JSON 文档更新、旧数据迁移改进和远程文件读取。',
          '**macos:** 新增 macOS 配置，并加入平台相关的 Header、子窗口和布局调整。',
          '**update-dialog:** 在更新对话框中支持以 Markdown 渲染发布说明。',
        ],
      },
      {
        title: '变更',
        items: [
          '**app-layout:** 重构 App 组件，引入新的布局组件，并简化 AppPanelContent 的面板渲染逻辑。',
          '**ai-settings:** 扩展 AI 模型列表和设置，简化文件大小设置，支持分组模型排序，并补充 AI 本地化内容。',
          '**ssh-form:** 将 SshForm 重构为用于代理、跳板机和双因素认证设置的标签页结构。',
          '**ui:** 改进 AIAssistantPanel、ModelCombobox、QuickCommands、操作按钮 tooltip 和思考文本样式。',
          '**deps:** 新增 react-markdown、remark-gfm、react-syntax-highlighter、browserslist、lightningcss，并更新相关依赖。',
          '**codebase:** 清理多个模块中的格式、导入顺序和函数签名。',
        ],
      },
      {
        title: '修复',
        items: [
          '**dialogs:** 为 dialog 和 alert dialog 遮罩层增加清理处理。',
          '**ai-assistant:** 改进 truncate_preview 字符串截断逻辑，并移除文本选择时的 toast 提示。',
          '**macos:** 修正 macOS 配置文件中的 titleBarStyle 大小写。',
          '**ssh-form:** 调整 SshForm 格式并整理对话框导入顺序。',
        ],
      },
      {
        title: '文档',
        items: ['更新配置存储文档，以说明基于 redb 的数据模型。', '扩展文档，补充 AI Assistant 功能和相关更新。'],
      },
    ],
  },
  {
    version: '[0.8.5] - 2026-04-28',
    sections: [
      {
        title: '新增',
        items: [
          '**session-sync:** 实现会话同步支持。',
          '**quick-commands:** 支持在 QuickCommands 中向所有用户发送命令。',
          '**release:** 新增用于修复 latest.json 和发布 updater 资产的工作流。',
        ],
      },
      {
        title: '变更',
        items: [
          '**ci:** 更新 build-release 工作流、资产修复下载脚本和发布资产上传流程。',
          '**docs:** 更新首页 URL，并在头部菜单中新增文档页面链接。',
          '**i18n:** 为英文和中文新增同步分组功能和菜单选项文案。',
        ],
      },
      {
        title: '修复',
        items: [
          '**ci:** 增强 build-release 工作流的缓存清理，新增 libudev-dev 构建依赖，并修复 GITHUB_TOKEN 缩进。',
          '**updater:** 新增 Tauri updater 签名密钥准备步骤，并改进 updater manifest 生成流程。',
        ],
      },
    ],
  },
  {
    version: '[0.8.4] - 2026-04-27',
    sections: [
      {
        title: '新增',
        items: [
          '**ssh:** 实现 HostKeyVerifyManager，用于主机密钥验证和 known_hosts 管理。',
          '**ssh:** 增强主机密钥验证日志，并加入验证超时机制。',
        ],
      },
      {
        title: '变更',
        items: ['**i18n:** 为英文和中文语言环境新增 SSH 主机密钥验证提示文案。'],
      },
      {
        title: '修复',
        items: ['**host-key-verification:** 新增 HostKeyVerifyDialog，并将主机密钥验证处理集成到应用中。'],
      },
      {
        title: '文档',
        items: ['更新 Docusaurus 配置以处理 broken anchors。'],
      },
    ],
  },
  {
    version: '[0.8.3] - 2026-04-27',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal:** 基于 shell integration 状态和终端模式实现命令建议可见性逻辑。',
          '**file-explorer:** 新增返回上级目录入口，并更新上下文菜单行为以改善导航体验。',
        ],
      },
      {
        title: '变更',
        items: ['**resource-monitor:** 增强资源监视器界面，并改进性能指标格式化展示。'],
      },
      {
        title: '文档',
        items: ['新增 CHANGELOG.md，用于记录 0.8.2 版本的重要变更。'],
      },
    ],
  },
  {
    version: '[0.8.2] - 2026-04-23',
    sections: [
      {
        title: '新增',
        items: [
          '**tauri:** 添加 Windows 配置文件，并移除未使用的 dragDropEnabled 属性。',
          '**file-transfer:** 增强文件传输处理以支持目录，包括目录传输的进度跟踪与界面更新。',
          '**session-management:** 实现按会话维度管理命令历史，包括获取、监听和清理命令历史，以提升使用体验。',
        ],
      },
      {
        title: '变更',
        items: [
          '**i18n:** 为英文和中文语言环境新增文件传输进度跟踪与完成提示文案。',
          '**header:** 更新窗口控制按钮，采用新图标并改进样式，以提升使用体验。',
        ],
      },
      {
        title: '修复',
        items: ['**saved-connections:** 为连接和分组项实现拖放支持，提升交互体验和组织能力。'],
      },
      {
        title: '性能',
        items: ['**file-explorer:** 通过记忆化和滚动处理增强 FileExplorer 组件，提升性能与使用体验。'],
      },
      {
        title: '文档',
        items: [
          '更新 README 和指南，补充 Windows 拖放支持、增强后的文件传输能力以及诊断设置等新特性说明，以提升使用体验。',
          '**file-transfer:** 优化拖放上传章节，使其在不同语言间更清晰且表述一致。',
        ],
      },
    ],
  },
  {
    version: '[0.8.1] - 2026-04-23',
    sections: [
      {
        title: '新增',
        items: [
          '**interaction:** 新增命令建议最小字符数限制设置及归一化逻辑，增强用户控制能力。',
          '**file-explorer:** 在 Windows 上使用 WebView2 实现外部文件拖放支持，增强拖拽交互能力。',
        ],
      },
      {
        title: '变更',
        items: [
          '**i18n:** 为英文和中文语言环境新增命令建议最小字符数限制相关文案，增强用户控制能力。',
          '**file-transfer:** 使用 useMemo 优化 visibleTransfers 的计算，以提升性能与排序表现。',
          '**terminal:** 用 useTerminalAppSettings 替换 useApp，以改进设置管理并保持终端组件间的一致性。',
          '**sync-backup:** 将按钮尺寸从 icon-xs 调整为 icon-sm，以提升界面一致性。',
          '**i18n:** 为英文和中文语言环境新增外部拖放支持提示文案，提升文件上传时的引导体验。',
        ],
      },
      {
        title: '文档',
        items: ['增强文档，补充会话导入导出、诊断和托盘支持等新特性说明，以提升清晰度和使用体验。'],
      },
    ],
  },
  {
    version: '[0.8.0] - 2026-04-22',
    sections: [
      {
        title: '新增',
        items: [
          '**interaction:** 新增命令建议最大字符数限制设置及归一化逻辑，提升对命令建议的控制能力。',
          '**quit_confirmation:** 实现 QuitConfirmDialog，在退出应用前请求用户确认，避免误关闭并提升使用体验。',
          '**tray:** 实现托盘功能，包括窗口管理和应用退出命令，提升使用体验。',
        ],
      },
      {
        title: '变更',
        items: [
          '**i18n:** 为英文和中文语言环境新增命令建议最大字符数限制相关文案，提升用户控制能力。',
          '**syncbackup:** 增强 SyncBackupHistoryPanel，加入新的 UI 组件、改进历史摘要逻辑，并增加额外筛选选项，以提升使用体验。',
          '**i18n:** 为英文和中文语言环境新增历史记录相关术语，提升清晰度与使用体验。',
          '**scrollbar:** 为滚动条角落添加透明背景，以提升 UI 一致性。',
          '**saved-connections:** 更新布局和样式，以提升响应式表现和视觉一致性。',
          '**settings:** 移除 ChildAppProvider 与 SettingsPage 中的 emit 调用，以简化事件处理。',
        ],
      },
    ],
  },
  {
    version: '[0.7.9] - 2026-04-21',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal:** 通过同步已渲染行中的输入状态并改进命令处理逻辑，增强终端输入处理能力。',
          '**syncbackup:** 实现 SyncBackup 功能及其管理云同步设置和历史记录的 UI 组件，提升备份管理体验。',
          '**security:** 新增主密码管理，并改进输入组件的动态状态处理，提升使用体验。',
          '**syncbackup:** 增加对 S3 endpoint 必填项的校验，并改进草稿设置的界面反馈，提升云同步管理体验。',
          '**otp:** 在 OtpDialog 中集成 input-otp 组件，改进 OTP 输入处理，并支持动态验证码长度。',
          '**cloud_sync:** 通过为 401 错误添加专门提示并改进存储错误映射，增强 WebDAV 认证的错误处理。',
          '**syncbackup:** 增强 SyncBackupHistoryPanel，加入筛选能力、改进状态管理并更新界面，以提升使用体验。',
        ],
      },
      {
        title: '变更',
        items: [
          '**terminal:** 移除未使用的输入同步逻辑，并简化命令清洗流程。',
          '**terminal:** 重命名命令跟踪函数，并增强命令注册逻辑以改进输入处理。',
          '**i18n:** 更新英文和中文语言文件，为同步与备份功能补充新文案并提升界面体验。',
          '**settings:** 重构设置页面，采用分类分组、改进滚动处理和动态标签管理，以提升使用体验。',
          '**i18n:** 更新 zh-CN 语言文件，新增同步与备份历史相关术语，增强筛选选项并优化提示文案。',
        ],
      },
      {
        title: '修复',
        items: ['**file-explorer:** 为文件浏览器实现会话缓存，在组件卸载后仍可保持状态，提升导航体验。'],
      },
      {
        title: '文档',
        items: ['增强同步与备份功能的文档和界面说明，包括详细指南、设置集成，以及跨设备配置与备份管理体验的改进。'],
      },
    ],
  },
  {
    version: '[0.7.8] - 2026-04-21',
    sections: [
      {
        title: '新增',
        items: [
          '**shell:** 实现终端输入的命令清洗，并新增终端命令工具函数。',
          '**session:** 通过引入 sendSessionInput 函数重构会话输入处理，改进跨组件的命令提交和预览管理。',
          '**logging:** 引入 console 使用 lint 规则，并增强多个组件中的错误日志结构，以提升诊断能力。',
          '**keywordhighlight:** 扩展关键词高亮中的错误和控制流模式，提升诊断能力。',
          '**quickcommands:** 实现 QuickCommandsStore，用于管理快捷命令的内存缓存与持久化，增强命令的写入与获取能力。',
        ],
      },
      {
        title: '变更',
        items: ['**observability, watcher, auth:** 对多个函数应用一致的格式和缩进，以提升代码可读性。'],
      },
      {
        title: '性能',
        items: ['通过为 AppContext、ChildAppProvider 和 TransferProvider 的上下文值使用 useMemo 来优化上下文提供者。'],
      },
    ],
  },
  {
    version: '[0.7.7] - 2026-04-15',
    sections: [
      {
        title: '新增',
        items: [
          '实现配置导入导出功能，并更新 ImportDialog 和 Header 组件的界面。',
          '**backup:** 新增带加密和轮换能力的配置导入导出功能。',
          '**connections:** 新增 OpenGroupConnectionsDialog 组件，并增强连接项交互，支持选择和上下文菜单操作。',
          '**panel:** 增强 QuickCommands 组件，改进搜索和分类筛选界面。',
        ],
      },
      {
        title: '变更',
        items: [
          '**i18n:** 更新英文和中文翻译，补充配置导入导出功能相关文案。',
          '**panel:** 更新 ActiveSessions 组件，改进搜索输入框和图标样式。',
          '**panel:** 调整 SavedConnections 组件中下拉菜单的宽度，以提升界面一致性。',
        ],
      },
    ],
  },
  {
    version: '[0.7.6] - 2026-04-15',
    sections: [
      {
        title: '新增',
        items: [
          '**ssh:** 改进 SSH 认证日志，并新增 known host 密钥校验。',
          '**ssh:** 增强 SSH I/O 循环，加入详细的退出状态和信号日志。',
        ],
      },
      {
        title: '变更',
        items: ["向 Cargo.toml 新增 'des' crate 依赖，并更新 Cargo.lock。"],
      },
      {
        title: '修复',
        items: ['恢复在 pty.rs 中对 SessionOutputCoalescer 的导入，以确保会话输出处理正常。'],
      },
      {
        title: '文档',
        items: [
          '更新 README，补充在线搜索、翻译和改进后的 SFTP 文件浏览器等新特性说明。',
          '增强文档，补充终端特性、文件传输能力和安全增强项（包括翻译支持与改进后的会话管理）的说明。',
        ],
      },
    ],
  },
  {
    version: '[0.7.5] - 2026-04-14',
    sections: [
      {
        title: '新增',
        items: [
          '**connection:** 增强会话连接处理，改进错误恢复和连接编辑提示。',
          '**ssh:** 增强 SSH 表单，加入密码管理和本地化更新。',
        ],
      },
    ],
  },
  {
    version: '[0.7.4] - 2026-04-14',
    sections: [
      {
        title: '新增',
        items: [
          '**updater:** 实现更新对话框和后台更新检查功能。',
          '**header:** 增强头部组件，加入更新检查功能和新图标。',
          '**terminal:** 为终端组件新增挂起状态处理和输出合并，在高负载下提升性能表现。',
        ],
      },
      {
        title: '变更',
        items: [
          '向 package.json 和 pnpm-lock.yaml 新增 @tauri-apps/plugin-process 与 @tauri-apps/plugin-updater 依赖。',
          '清理导入并改进多个组件的格式，以提升可读性。',
          '**i18n:** 为英文和中文语言环境新增更新器本地化文案，包括更新状态消息。',
          '**i18n:** 为英文和中文语言环境新增大输出保护相关文案。',
        ],
      },
    ],
  },
  {
    version: '[0.7.3] - 2026-04-14',
    sections: [
      {
        title: '新增',
        items: [
          '**keywordhighlightpresets:** 扩展成功匹配模式，加入更多关键词以提升匹配效果。',
          '**connection-management:** 实现连接失败时的错误处理，支持将标签页和窗格标记为失败，同时保持布局完整。',
          '**file-explorer:** 实现目录历史管理并增强选择处理。',
          '**file-transfer:** 新增文件传输的暂停、继续和取消功能，并更新相关上下文与界面组件。',
        ],
      },
      {
        title: '变更',
        items: [
          '**i18n:** 为英文和中文语言环境新增连接失败提示文案。',
          '**file-explorer:** 更新选择处理方法并改进上下文菜单交互。',
          '**i18n:** 更新英文和中文语言环境中的文件传输操作文案，包括取消、暂停、继续和删除。',
        ],
      },
    ],
  },
  {
    version: '[0.7.2] - 2026-04-14',
    sections: [
      {
        title: '新增',
        items: [
          '**interaction-settings:** 在 InteractionTab 中新增命令建议开关，并接入应用设置。',
          '**logging:** 实现 warn 和 error 级别的持久化日志，并新增对应的 Tauri 命令来处理日志写入。',
          '**file-explorer:** 增强键盘交互，加入删除功能和文件列表焦点管理。',
          '**sftp:** 增强远程文件操作，加入更详细的日志和权限处理。',
        ],
      },
      {
        title: '变更',
        items: [
          '**file-explorer:** 用本地库替换 invoke 导入，并为删除按钮新增 autoFocus，以提升可访问性。',
          '**file-explorer:** 在多个对话框组件中统一将 invoke 导入替换为本地库，以保持一致性。',
          '**i18n:** 为英文和中文语言环境新增命令建议相关文案。',
        ],
      },
      {
        title: '修复',
        items: ['**keywordhighlightpresets:** 更新 duration 正则表达式，使其支持简写单位，提升匹配效果。'],
      },
      {
        title: '文档',
        items: ['更新 CLAUDE.md 和 README.md，澄清文档站点的构建与服务命令，包括按语言环境热更新的选项。'],
      },
    ],
  },
  {
    version: '[0.7.1] - 2026-04-13',
    sections: [
      {
        title: '新增',
        items: [
          '**clipboard:** 实现 readClipboardText 函数，并更新终端组件以使用它访问剪贴板。',
          '**demos:** 新增多种演示脚本，用于展示 NyaTerm 的终端特性，包括动作链接、文件监听、关键词高亮和结构化输出。',
          '**activesessions:** 增强 ActiveSessions 组件，加入搜索功能、会话重连/断开操作，并改进会话展示界面。',
          '**file-explorer:** 重构 DeleteDialog 以处理多文件删除，并改进界面；同时更新 FileExplorer 以支持批量删除操作。',
          '**resource-monitor:** 实现刷新按钮，并使用 async/await 改进统计信息获取流程；同时增加加载状态管理。',
          '**modal-management:** 重构模态子窗口处理逻辑，改进焦点强制和状态跟踪；并在 ActiveSessions 组件中加入会话重连和断开功能。',
          '**activesessions:** 简化 PanelHeader 操作区，移除用于会话数量展示的多余包裹 div。',
          '**resource-monitor:** 为刷新按钮增加 tooltip，并重命名状态变量以提升可读性。',
        ],
      },
      {
        title: '变更',
        items: ['**i18n:** 更新 zh-CN 和 en.json，补充活动会话和文件删除提示文案。'],
      },
      {
        title: '文档',
        items: [
          '更新 README 和用户指南，增强对 NyaTerm 功能、会话类型和终端能力的说明，并新增工作区布局、安全和网络配置等章节。',
          '**sidebars:** 更新指南章节，加入会话类型、布局和认证等主题，并重新组织现有条目以提升清晰度。',
        ],
      },
    ],
  },
  {
    version: '[0.7.0] - 2026-04-12',
    sections: [
      {
        title: '新增',
        items: [
          '增强终端工作区，加入新的标签页管理和窗格功能。',
          '**crypto:** 实现主密码包裹密钥加密体系。',
          '**app:** 在应用启动时恢复主密码的加密状态。',
          '**config:** 引入 proxy_jump_id 字段和循环依赖校验。',
          '**ssh:** 通过 direct-tcpip channel 实现多跳 proxy jump 路由。',
          '**ui:** 在 SSH 会话对话框中集成跳板机配置。',
          '**shell:** 将串口发送器升级为统一的 shell 命令广播器。',
          '**explorer:** 将文件浏览器限制为仅在 SSH 会话中使用，并显示不支持提示。',
          '**tabbar:** 新增带呼吸动画的未读指示器，并扩展 TabBarProps。',
          '**unreadtracking:** 实现会话未读输出跟踪，并更新 TabWindowsWorkspace 以显示未读标签页 ID。',
          '**terminal:** 新增 TerminalGutter 组件用于显示行号和时间戳，并将设置中的动作链接默认关闭。',
        ],
      },
      {
        title: '变更',
        items: [
          '**window:** 在 tauri 配置中启用透明窗口背景。',
          '**ssh:** 将默认 keepalive 间隔从 60 秒降低到 3 秒。',
          '**config:** 格式化 ui 配置中的元组结构。',
          '**security:** 将 lock_password 迁移到统一的 master_password 定义。',
          '**ssh:** 将单一 session handle 解耦为多层 SshConnectionHandles。',
          '**panel:** 将 QuickCommands 和 SerialSendPanel 迁移到 panel 模块。',
          '**ui:** 移除旧的全屏快捷键和冗余菜单项。',
          '**panel:** 调整活动会话数量指示器的格式。',
          '提交剩余更改。',
          '**keywordhighlight:** 更新 token 边界处理，消除冲突。',
          '**i18n:** 为终端设置新增行号和时间戳选项。',
        ],
      },
      {
        title: '修复',
        items: [
          '**otp:** 正确解码 URL 编码中的多字节 UTF-8 字符。',
          '**ssh:** 防止提示注入脚本污染 shell 历史记录。',
          '**session:** 在关闭会话期间静默忽略 not-found 错误。',
          '**terminal:** 在附加到即将终止的会话时抑制错误。',
          '**terminal:** 当没有活动建议或选择项时，避免错误地关闭建议列表。',
          '**settings:** 默认禁用终端设置中的关键词高亮和动作链接。',
        ],
      },
      {
        title: '性能',
        items: ['仅在成功关闭后再从 UI 中移除工作区标签页。', '使分屏窗口中的会话放置逻辑更加明确。', '减少终端工作区中的不必要重渲染。'],
      },
      {
        title: '文档',
        items: ['新增 CLAUDE.md，提供开发指南和架构概览。'],
      },
    ],
  },
  {
    version: '[0.6.1] - 2026-04-11',
    sections: [
      {
        title: '变更',
        items: ['更新 sync-version 脚本中的版本同步逻辑。', '将 nyaterm 依赖版本更新为 0.6.0。'],
      },
    ],
  },
  {
    version: '[0.6.0] - 2026-04-11',
    sections: [
      {
        title: '新增',
        items: [
          '**proxy:** 新增独立的代理与隧道管理。',
          '**sftp:** 增强文件传输，支持并发、重试和时间戳。',
          '**ui:** 实现网络面板和设置重构。',
          '实现用于安全管理应用设置和验证密码的 Tauri 命令。',
          '**network:** 增强隧道配置界面。',
          '新增会话录制和自定义传输偏好设置。',
          '**ui:** 新增 OtpDialog 以支持双因素认证。',
          '**core:** 实现与 PendingAuthManager 和命令的 OTP 交互。',
          '**ui:** 实现 OSC7 CWD 跟踪支持和相关 UI 禁用状态。',
          '**ui:** 将 OtpDialog 集成到主应用布局中，并支持 i18n。',
          '**transfer:** 支持从传输底栏打开下载路径。',
          '**security:** 新增标签页数量显示，并更新 Key/Password 管理页签以显示数量。',
          '**ssh-form:** 增强 SSH 表单，加入代理和 OTP 配置选项。',
          '**otp:** 实现 OTP 管理及其与 UI 组件的集成。',
          '**prettier:** 新增用于 JSON 排序的 Prettier 配置，并更新 i18n 检查脚本。',
          '**search:** 为 SearchEngine 新增 show_in_menu 属性，并增强 SearchTab，加入可折叠的自定义引擎界面。',
          '**session:** 按类型启动本地、Telnet 和串口连接。',
          '**serial:** 在会话编辑器中显示检测到的串口。',
          '**serial:** 新增底部串口发送面板。',
        ],
      },
      {
        title: '变更',
        items: [
          '**ui:** 引入 shadcn UI 组件。',
          '**i18n:** 更新网络和传输功能的翻译。',
          '**translate:** 对 translate API 的模块依赖进行小幅更新。',
          '格式化会话代理相关导入。',
          '调整面板头部操作区布局。',
          '**deps:** 将 russh 升级到 0.60。',
          '**ui:** 将 saved-connections 对话框目录重命名为 connections。',
          '**core:** 重组 ssh、runtime 和 import 的模块结构。',
          '更新内部导入并完成 ssh 模块提取。',
          '**core:** 采用新的 ssh 和 runtime 模块结构。',
          '**ui:** 更新 Header 中针对新 connections 目录的导入路径。',
          '重构命令模块并更新导入路径，以提升组织性。',
          '**config:** 重命名存储模块并拆分 settings 配置。',
          '**runtime:** 提取 tauri 启动流程和命令适配器。',
          '**core:** 提取 history store 并统一 error 导入。',
          '**session-dialog:** 使新建会话表单具备更好的响应式布局。',
          '**dialog:** 改进快捷命令和自动上传布局。',
          '**settings:** 引入响应式设置外壳。',
          '**settings-search:** 重新布局自定义搜索引擎编辑器。',
          '**settings-terminal:** 重新布局动作链接和高亮编辑器。',
          '**panel:** 优化移动端面板和认证页签。',
          '**core:** 导出 watcher 模块。',
          '**rust:** 统一后端格式。',
          '**i18n:** 规范英文排序标签。',
          '**otp:** 内置本地 hotp 和 totp crate。',
          '**format:** 移除尾随空白。',
          '**format:** 去除 translate core 中末尾多余空行。',
          '**quick-commands:** 清理格式并改进 tooltip 组件结构。',
          '**resource-monitor:** 改进代码格式和结构，以提升可读性。',
          '**settings:** 重构设置组件，使用 SettingSection 来提升组织性和可读性。',
          '重组文件浏览器、认证和 save-connections 组件。',
          '**connection:** 将已保存连接 schema 规范化为类型化配置块。',
          '**saved-connections:** 提取带 tooltip 的头部操作按钮。',
          '**file-explorer:** 在工具栏中复用带 tooltip 的图标按钮。',
          '**i18n:** 移除已弃用的默认本地 shell 标签。',
          '**frontend:** 规范面板导入并进行小幅清理。',
          '**rust:** 隔离导入重排和换行调整。',
          '**file-explorer:** 为对话框导入增加包裹层以保持一致性。',
          '引入 FileUploadPage，并更新路由以替换 AutoUploadPage。',
        ],
      },
      {
        title: '修复',
        items: [
          '**ssh:** 使用 Mutex 解决并发访问 SshHandler 的问题。',
          '**security:** 为临时目录能力增加 app 作用域。',
          '**ui:** 处理关键词高亮缓存中的 xterm 缓冲区裁剪问题。',
          '**i18n:** 修正多个 UI 文案的中文翻译。',
          '**explorer:** 在同步目录之前规范化 cwd 路径。',
          '**panel:** 将 SecurityAuthPanel 的默认标签从 passwords 调整为 keys。',
          '**ssh:** 为 PowerShell 的 OSC 集成使用字符转义。',
          '**select:** 允许触发器内容在窄布局中收缩并截断。',
          '**session-ui:** 限制仅 SSH 会话显示相关面板，并明确路径同步提示。',
          '**session-editor:** 在清空表单时重置本地终端默认值。',
          '**i18n:** 更新串口相关文案，并恢复 serial send 的本地化支持。',
        ],
      },
    ],
  },
  {
    version: '[0.5.0] - 2026-04-07',
    sections: [
      {
        title: '新增',
        items: [
          '**window:** 实现子窗口模态管理和遮罩层。',
          '**auth:** 为 SSH 会话新增托管密码存储。',
          '**stats:** 为 SSH 会话新增远程资源监视器。',
          '**sftp:** 新增递归目录传输命令。',
        ],
      },
      {
        title: '变更',
        items: ['更新标签页边框和阴影样式。', '**ui:** 采用活动栏布局和自定义窗口 chrome。'],
      },
      {
        title: '修复',
        items: ['**i18n:** 优化中文语言环境中实验性关键词高亮描述。', '**terminal:** 在断开后重新连接 SSH 标签页。'],
      },
    ],
  },
  {
    version: '[0.4.0] - 2026-04-03',
    sections: [
      {
        title: '新增',
        items: [
          '实现 ChildWindowRouter，并在支持 i18n 的基础上增强窗口管理。',
          '增强关键词高亮设置及功能。',
          '更新交互设置中的分词分隔符，以提升解析效果。',
          '增强文件传输功能和加载状态管理。',
          '为 TerminalTab 新增折行关键词高亮设置。',
          '**session:** 在新建会话表单中新增多协议标签页。',
          '**file-explorer:** 在子窗口中打开自动上传提示。',
          '**appearance:** 支持独立终端主题和字体缩放。',
          '**terminal:** 新增可操作链接和悬浮菜单。',
        ],
      },
      {
        title: '变更',
        items: [
          '更新项目 URL 并增强构建脚本。',
          "**i18n:** 为英文和中文翻译新增 'Built-in' 字体标签。",
          '**ui:** 优化标签页外观并刷新连接图标。',
        ],
      },
      {
        title: '修复',
        items: ['**app:** 稳定活动标签页状态和终端默认值。', '**keywordhighlight:** 改进内置匹配和单元格映射。', '**build:** 对齐 Vite 类型和路径别名设置。'],
      },
      {
        title: '文档',
        items: ['新增支持双语的 Docusaurus 文档站点。', '重新设计首页并修复 i18n 问题。'],
      },
    ],
  },
  {
    version: '[0.3.5] - 2026-03-09',
    sections: [
      {
        title: '修复',
        items: ['**keywordhighlight:** 增强日期时间和数字模式，以提升匹配精度。'],
      },
    ],
  },
  {
    version: '[0.3.4] - 2026-03-09',
    sections: [
      {
        title: '变更',
        items: ['**terminal:** 将 kbd 元素替换为 Kbd 组件，以在 CommandSuggestions 和 ContextMenu 中保持一致性。'],
      },
    ],
  },
  {
    version: '[0.3.3] - 2026-03-09',
    sections: [
      {
        title: '新增',
        items: [
          '**terminal:** 新增关键词高亮功能。',
          '**connections:** 为连接项上下文菜单新增编辑选项。',
          '**settings:** 支持跳转到特定设置标签，并在获得焦点时自动刷新 SSH 密钥。',
          '**shortcuts:** 为终端和 UI 操作实现全局键盘快捷键。',
        ],
      },
      {
        title: '变更',
        items: [
          '同步 Cargo.lock 中的版本，并更新提交文件列表。',
          '**terminal:** 改进 TabBar 关闭按钮的界面和悬停状态。',
          '**terminal:** 终端 ref 改用 React.RefObject，替代 MutableRefObject。',
          '**theme:** 更新 githubLight 和 nordLight 主题下的终端光标颜色。',
        ],
      },
      {
        title: '修复',
        items: ['**terminal:** 在硬件加速切换时重新初始化 WebGL addon。', '**ssh:** 防止 OSC 7 注入污染 bash 历史记录。'],
      },
    ],
  },
  {
    version: '[0.2.1] - 2026-03-06',
    sections: [
      {
        title: '新增',
        items: [
          '**session-management:** 增强会话处理，加入自动连接功能。',
          '**types:** 为会话管理和 UI 配置新增完整的全局类型。',
          '**file-explorer:** 新增用于创建文件、文件夹和符号链接的对话框。',
          '**translate:** 实现 Google Translate 的动态 TKK 生成。',
          '**file-explorer:** 实现终端路径同步功能。',
        ],
      },
      {
        title: '变更',
        items: [
          '将 themes 和 types 迁移到 lib 目录。',
          '更新 `.gitignore`，加入更多文件模式。',
          '更新导入路径并增强翻译设置。',
          '更新到全局类型的导入路径。',
          '**icons:** 统一文件图标逻辑并增强图标导入。',
        ],
      },
    ],
  },
  {
    version: '[0.1.5] - 2026-03-06',
    sections: [
      {
        title: '新增',
        items: [
          '**ui:** 实现缩放级别持久化和视图设置。',
          '**ui:** 在关于对话框中加入可点击的首页和问题反馈链接。',
          '**ui:** 增强头部菜单，加入图标以及新的文档和日志帮助选项。',
          '**logging:** 增强 tracing 初始化，加入滚动文件 appender，并更新日志权限。',
          '**window:** 在启动时显示应用窗口，并更新 tauri 配置以允许窗口可见。',
          '**connections:** 新增用于分组管理 SSH 连接的 SavedConnections 面板。',
          '**watcher:** 新增文件监听支持和分块文件传输进度跟踪。',
          '**file-explorer:** 集成自定义对话框和上下文菜单支持。',
          '**settings:** 实现全局设置对话框和本地化。',
          '**terminal:** 新增终端上下文菜单工具和搜索栏。',
          '**security:** 新增锁屏和锁屏密码加密。',
          '**quick-commands:** 重新设计快捷命令界面，支持图标和变量。',
          '**file-transfer:** 新增文件属性对话框和传输进度条。',
          '**settings:** 新增翻译设置以及标签页式的设置/关于体验。',
          '**translate:** 新增 TranslationTab 和多提供商翻译服务。',
          '**terminal:** 增强 XTerminal，支持打开 URL 并改进命令历史处理。',
          '**app:** 引入全局应用上下文，并扩大 i18n 覆盖范围。',
          '**search:** 新增搜索引擎图标，并改进 SearchTab 的配置界面。',
          '**import:** 新增从 Xshell、MobaXterm 和 WindTerm 导入会话的功能。',
          '**ui:** 新增命令面板、popover 和可拖拽面板组件。',
          '**icons:** 扩展图标系统并更新类型定义。',
          '**connections:** 增强连接处理、反馈、排序和拖拽能力。',
          '**config:** 新增锁屏和连接排序模式设置。',
          '**security:** 实现锁屏开关和空闲检测。',
          '**suggestions:** 增强命令建议功能，支持多提供商。',
          '**event-listeners:** 使用事件监听替代轮询，以获取会话和命令历史更新。',
        ],
      },
      {
        title: '变更',
        items: [
          '新增 MIT License 文件。',
          '**assets:** 更新应用图标和 logo 资源，并移除未使用的 SVG。',
          '**cleanup:** 更新 tauri 配置并移除未使用资源。',
          '**i18n:** 在整个应用中集成 i18next。',
          '**ui:** 将页面标题从 `NyaTerm Terminal` 更新为 `NyaTerm`。',
          '更新滚动条样式。',
          '更新全局 UI、布局可见性和主题配置。',
          '采用 shadcn/ui 组件。',
          '将 toast 通知迁移到 sonner，并使用 shadcn 上下文菜单。',
          '将设置对话框更新为使用开关和标签页界面。',
          '**i18n:** 更新新组件和功能的本地化文案。',
          '更新排版、CSS 变量、主题颜色和章节标题。',
          '更新依赖、共享工具、类型、UI 组件和面板。',
          '**backend:** 将配置和命令模块拆分为子模块。',
          '**theme:** 使用 CSS 变量和预设主题重构主题系统。',
          '**dialog:** 将对话框重组到按领域划分的子目录中。',
          '**settings:** 为新的配置结构更新设置页签。',
          '**app:** 刷新 App、contexts、布局和面板组件。',
          '**i18n:** 为新增的设置和 UI 流程补充语言键。',
          '**window:** 将对话框迁移为独立子窗口。',
          '**file-explorer:** 模块化文件树并替换原生对话框。',
          '**terminal:** 清理格式和多余空白。',
          '**tracing:** 改进本地时间格式，并移除内联密钥迁移逻辑。',
          '**dialogs:** 移除 NewSessionDialog、SettingsDialog 和 QuickCommandDialog。',
          '**components:** 提取设置组件并统一导入路径。',
          '从仓库中删除生成的构建产物。',
          '将版本提升到 `0.1.5`，并新增版本同步脚本。',
        ],
      },
      {
        title: '修复',
        items: [
          '修复对话框可访问性警告。',
          '更新 SearchTab 中翻译键的用法，使设置说明更清晰。',
          '改进会话处理和界面响应性。',
          '**settings:** 更新复制和粘贴的默认交互设置。',
          '**translations:** 移除对话框和组件中翻译键的 fallback 值。',
        ],
      },
      {
        title: '性能',
        items: ['**sftp,ssh:** 优化传输速度并新增符号链接支持。'],
      },
      {
        title: '文档',
        items: ['更新 README，补充关键特性和使用说明。', '移除 README 标语末尾的句号。'],
      },
    ],
  },
];

const changelogReleasesByLocale: Record<string, ChangelogRelease[]> = {
  en: changelogReleasesEn,
  'zh-CN': changelogReleasesZhCN,
};

export function getChangelogReleases(locale: string): ChangelogRelease[] {
  return changelogReleasesByLocale[locale] ?? changelogReleasesByLocale['zh-CN'];
}

# Themes & Appearance

NyaTerm lets you tune the workspace appearance in fairly fine detail, including UI theme, terminal theme, fonts, and cursor behavior.

## UI theme and terminal theme

In **Settings → Appearance**, you can configure these separately:

- **UI Theme** — controls the app-wide color scheme
- **Terminal Theme** — controls terminal colors, or can follow the UI theme

If you just want a quick theme switch, you can also use **View → Theme** from the top menu.

The built-in **WindTerm Dark** (`windterm-dark`) and **WindTerm Light** (`windterm-light`) themes are based on [WindTerm](https://github.com/kingToolbox/WindTerm)'s `dige-black` and `dige-white` palettes. They retain the 16 ANSI colors and adapt UI state colors for NyaTerm. Select either independently for the UI or terminal, or let the terminal follow the UI. To see the original terminal colors, turn off minimum contrast enhancement below; some ANSI colors in the light theme have low contrast on white.

The built-in **Dracula** (`dracula`) and **Dracula Light** (`dracula-light`) follow the [official Dracula specification](https://draculatheme.com/spec): the dark theme uses the Dracula Classic palette and the light theme uses the official Alucard Classic (cream background). The specification defines a palette but no light ANSI mapping, so the light theme keeps ANSI black dark so that black text stays visible on the light background. The dark theme's bright black (`#6272a4`) is fairly dim; turn on minimum contrast if you need it more legible.

The theme list includes two high-contrast themes, suited for accessibility, bright environments, or screen sharing:

- **Nya HC** — high contrast on a dark base
- **Nya HC White** — high contrast on a light base

Select them like any other UI theme from **Settings → Appearance** or the **View → Theme** top menu.

## Custom themes

Open **Settings → Appearance → Manage themes** to browse built-in and custom themes. Selecting a theme changes only the local preview. Built-in themes are read-only; create a copy to edit them. Custom themes support editing, renaming, duplication, export, and deletion.

The editor separates UI and terminal colors, with visual pickers and color inputs. The preview includes UI controls, terminal examples, and the 16 ANSI colors. Invalid inputs keep the last valid preview; low contrast produces a warning.

**Save changes** persists the theme immediately. Saving an active theme refreshes every place using it. **Apply to UI / terminal** changes that scope only; **Apply to both** makes the terminal follow the UI. Unsaved edits can be saved and applied in one operation. Canceling settings does not undo confirmed manager operations. Appearance selectors and other settings still use the settings **Apply / Cancel** workflow.

Creation, duplication, and import produce drafts until saved. Leaving an edited draft offers Save, Discard, or Keep editing. Deletion requires confirmation: an affected UI falls back to GitHub Dark, and terminals explicitly using the deleted theme follow the UI. Deletion cannot be undone.

JSON import and export remain compatible with Tauri single-theme files. Conflicting imported IDs receive a new ID. Exporting a valid draft does not save or apply it. Unknown fields and unsupported records are preserved. Concurrent edits produce a conflict while preserving the draft; export it, or discard and reload before retrying.

## Minimum contrast

**Settings → Appearance → Minimum contrast** automatically adjusts the terminal foreground color when it sits too close to the background, so a remote program's color choices stay readable under your current theme. Available steps:

| Step | Meaning |
|------|---------|
| Off | No adjustment; colors render exactly as the remote sends them |
| Slight boost | At least 3:1 contrast |
| Recommended: WCAG AA | At least 4.5:1 contrast |
| High contrast: WCAG AAA | At least 7:1 contrast |
| Maximum | At least 21:1 contrast, i.e. forced to pure black or white |

It only affects how terminal output is rendered; it does not change the theme's own color definitions.

## Background image

In **Settings → Appearance**, the main window can use a local wallpaper:

- **Background Image** — choose the local file rendered behind the main workspace
- **Image Sizing** — choose how the image is shown with `cover`, `contain`, `stretch`, or `tile`
- **Image Opacity** — control how strongly the wallpaper shows through the theme
- **Background Content Opacity** — control how translucent workspace panels and content surfaces become; lower values make the wallpaper more visible

This only affects the main window workspace. Settings and child windows stay solid so forms, dialogs, and secondary windows remain readable.

## Fonts and font size

In **Settings → Appearance**, you can adjust:

- **Font family** — primary font plus multi-level fallback fonts
- **Terminal font size**
- **UI font size**

The default font family is `JetBrains Mono, Noto Sans SC Variable, Inter`: the terminal falls back to `JetBrains Mono` and the UI falls back to `Inter`.

These are **font names, not font files shipped with the app**. NyaTerm resolves them from your system-installed fonts, so on a machine without `JetBrains Mono` the terminal falls back to the platform's default monospace font. If you want this exact stack, install the fonts yourself.

The font picker lists system-installed fonts so you can build a fallback chain, and the terminal font dropdown only offers families that measure as monospace. System font discovery runs asynchronously, so you may briefly see `Loading system fonts...` when opening the picker; you can still type a font name in the meantime.

## Cursor

Appearance settings also expose terminal details such as:

- **Cursor style** — Block / Underline / Bar
- **Cursor blink**

If you switch between dark and light themes often, it is worth checking the terminal theme together with keyword highlighting and action links so the overall result stays readable.

## Language switching

NyaTerm currently provides:

- Simplified Chinese
- English

You can switch language in either of these places:

- **Settings → General → Language**
- **View → Language** in the top menu

## Panels and workspace appearance

Besides colors and fonts, the workspace itself can be tuned to match your habits:

- Left and right panel widths are resizable
- Split ratios inside a tab are resizable
- Left and right activity bars can be shown or hidden quickly with shortcuts

These layout states are saved with app settings, which makes it practical to keep a preferred long-term workspace arrangement.

## Zoom and quick adjustments

NyaTerm provides these common shortcuts:

- **Zoom In** — `Ctrl / Cmd + =`
- **Zoom Out** — `Ctrl / Cmd + -`
- **Reset Zoom** — `Ctrl / Cmd + 0`

Use them to adjust readability during demos, screen sharing, or on high-DPI displays.

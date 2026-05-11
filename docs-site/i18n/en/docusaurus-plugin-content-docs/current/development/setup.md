---
sidebar_position: 2
---

# Development Setup

## Prerequisites

### Node.js

Install Node.js v18 or newer. Recommended: use [nvm](https://github.com/nvm-sh/nvm) (Linux/macOS) or [nvm-windows](https://github.com/coreybutler/nvm-windows) (Windows).

Use [pnpm](https://pnpm.io/) as the package manager.

### Rust

Install the latest stable Rust:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Windows users: visit [rustup.rs](https://rustup.rs/).

### Platform-Specific Dependencies

#### Windows

Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with "Desktop development with C++".

#### macOS

```bash
xcode-select --install
```

#### Linux (Ubuntu/Debian)

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

## Get the Source

```bash
git clone https://github.com/nyakang/nyaterm.git
cd nyaterm
```

## Install Dependencies

```bash
pnpm install
```

## Start Development

```bash
pnpm tauri dev
```

This starts both:
- Vite dev server (port 1420, HMR port 1421)
- Tauri application window

Frontend changes hot-reload; Rust changes trigger recompilation.

## Production Build

```bash
pnpm tauri build
```

Build artifacts are in `src-tauri/target/release/bundle/`.

## Available Scripts

| Command | Description |
|---------|-------------|
| `pnpm dev` | Start Vite dev server only |
| `pnpm build` | TypeScript check + Vite build |
| `pnpm tauri dev` | Start Tauri development mode |
| `pnpm tauri build` | Build for production |
| `pnpm lint` | Run Biome linting |
| `pnpm format` | Run Biome auto-format |
| `pnpm version-sync` | Sync version numbers across files |
| `pnpm --dir docs-site start` | Start the docs site for all locales |
| `pnpm --dir docs-site start:zh` | Start the Chinese docs dev server |
| `pnpm --dir docs-site start:en` | Start the English docs dev server |
| `pnpm --dir docs-site build` | Build the docs site |

## Docs workflow tips

If you are editing README or files under `docs-site/docs/` / `docs-site/i18n/en/`, it is a good idea to run the docs build so you can verify:

- both locales still build successfully
- new pages appear in navigation
- relative links still resolve correctly

## Code Style

The project uses [Biome](https://biomejs.dev/) for linting and formatting:

```bash
pnpm lint    # Check
pnpm format  # Auto-format
```

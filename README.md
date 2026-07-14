# GIF Library

GIF Library is a local-first Windows desktop app for saving and organizing GIFs, images, and remote media links.

## Import Files Only From People You Trust

Backup/import files can contain media data, links, notes, tags, aliases, and folder names. Only import files from trusted users.

It is built with Tauri, React, TypeScript, Rust, and SQLite. Library data is stored locally on your computer, and imported local files may be copied into app-managed local storage.

## Features

- Save remote media links.
- Import local GIF, PNG, JPG/JPEG, and WEBP files.
- Organize media with folders, tags, aliases, and notes.
- Search globally or within the current view.
- Copy links or local files to the clipboard.
- Import and export backups.
- Use local cleanup tools for duplicate records and static previews.

## It Does Not

- connect to Discord
- use Discord tokens
- automate Discord
- scrape private APIs
- upload your files to the cloud
- include telemetry
- run as a background service

## Install

Use the Windows installer from Releases.

Builds are unsigned, so Windows SmartScreen may show a warning.

## Basic Usage

1. Add a remote URL or local file.
2. Organize items with folders, tags, aliases, and notes.
3. Left-click a media card to copy the best available representation: a link for link-backed items, or the file for local-only items.
4. Export backups before wiping the library or uninstalling the app.

## Development

Prerequisites:

- Node.js with npm
- Rust stable with Cargo
- Microsoft C++ Build Tools with the Desktop development with C++ workload
- Microsoft Edge WebView2 Runtime

Commands:

```powershell
npm install
npm run check
npm run build
npx tauri build
```

## Privacy And Security

See [PRIVACY.md](PRIVACY.md) for data-handling notes and [SECURITY.md](SECURITY.md) for vulnerability reporting.

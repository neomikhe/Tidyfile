# Tidyfile

A free, open-source, cross-platform file organizer with a graphical interface.

Define visual rules — *"if it's a PDF and the name contains 'invoice', move it to Documents/Invoices/2026 and rename it with the date"* — and Tidyfile watches your folders and keeps them tidy on its own.

Everything runs locally. No cloud, no telemetry, no subscription.

> **Status: early development.** The project is in its setup phase and is not usable yet. There are no releases available.

## Why

Existing tools each miss something: Hazel is paid and macOS-only, File Juggler is paid and Windows-only, DropIt has had no releases since 2018, and `organize` and `hazelnut` are terminal tools aimed at technical users.

Tidyfile aims to be the option that is free, open source, cross-platform *and* graphical at the same time — including a real GUI on Linux.

## Planned features

- **Visual rule editor** — combinable conditions (AND/OR) and actions, no YAML or regex required.
- **Real-time folder watching**, plus manual and scheduled runs.
- **Simulation first** — every new rule shows exactly which files it would affect before it does anything.
- **Undo history** — every action is journaled; one click reverts it.
- **Safe by design** — files go to the trash, never deleted directly.

## Stack

Tauri 2 · Rust · Svelte · TypeScript · SQLite

## Building from source

Requires [Rust](https://rustup.rs) 1.85+ and [Node.js](https://nodejs.org) 22+, plus the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your platform.

```bash
npm install
npm run tauri dev      # run in development
npm run tauri build    # produce installers
```

## License

[GNU General Public License v3.0](LICENSE).

Tidyfile exists because the good tools in this category are paid and closed. The GPL keeps it that way round: anyone may use, study, modify and share it, but a distributed derivative has to ship its source under the same terms. Nobody gets to close it and sell it back to you.

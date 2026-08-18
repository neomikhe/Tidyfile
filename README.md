# Tidyfile

A free, open-source, cross-platform file organizer with a graphical interface.

Define visual rules — *"if it's a PDF and the name contains 'invoice', move it to Documents/Invoices/2026 and rename it with the date"* — and Tidyfile watches your folders and keeps them tidy on its own.

Everything runs locally. No cloud, no telemetry, no subscription.

> **Status: pre-release.** The app works end to end — rules, preview, tidying, undo and live watching — but it has not been through a beta yet and there are no published releases. Build it from source if you want to try it.

## Why

Every existing tool misses something:

| | Price | Cross-platform | GUI | Open source | Maintained |
|---|---|---|---|---|---|
| Hazel | $42 | macOS only | Yes | No | Yes |
| File Juggler | ~$40 | Windows only | Yes | No | Yes |
| File Arbor | Freemium | Win / macOS | Yes | No | Yes |
| DropIt | Free | Windows only | Yes | Yes | No releases since 2018 |
| organize | Free | Yes | No — CLI + YAML | Yes | Yes |
| hazelnut | Free | Yes | No — terminal UI | Yes | Yes |
| **Tidyfile** | **Free** | **Win / macOS / Linux** | **Yes** | **Yes** | **Yes** |

Nothing else ticks every box, and nothing else offers a real graphical interface on Linux.

## Your files are the product

A file organizer that loses someone's files is worthless, so safety is not a feature here — it is the design.

- **Nothing is ever deleted.** Removals go to the system trash. There is no direct delete anywhere in the codebase, and the test suite enforces it.
- **Preview before anything moves.** Every rule shows exactly which files it would touch, and what would happen to each. The preview and the real run share the same code, so they cannot disagree.
- **Everything is undoable.** Each operation is written to a SQLite journal *before* the disk is touched, then marked done. One click reverts a batch — including batches applied automatically while watching.
- **Nothing is overwritten.** A name collision gets a numbered suffix. Existing files are never replaced.
- **Interruptions are survivable.** If the app is killed mid-run, the journal records what was left unconfirmed.

## Features

- **Visual rule editor** — combinable conditions (all/any) and actions. No YAML, no mandatory regex.
- **Conditions** — extension, name contains, glob, regex, size, age, subfolder.
- **Actions** — move, copy, rename with templates, send to trash, and automatic subfolders by date or type.
- **Live watching** — new files are tidied as they arrive, with debouncing so partial downloads are left alone.
- **History** — every batch listed, every batch undoable.

Rename and subfolder templates accept `{name}`, `{ext}`, `{date}`, `{year}`, `{month}`, `{day}` and `{counter}`.

## Building from source

Requires [Rust](https://rustup.rs) 1.85+ and [Node.js](https://nodejs.org) 22+, plus the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your platform.

```bash
npm install
npm run tauri dev      # run in development
npm run tauri build    # produce installers
```

## Unsigned builds

Releases are not code-signed: an Apple certificate costs $99/year and a Windows one costs more, which is not sustainable for a free project right now.

This means macOS and Windows will warn you the first time you open the app. Published SHA-256 checksums let you verify a download matches what CI built. Signing may come later if the project warrants it.

## License

[GNU General Public License v3.0](LICENSE).

Tidyfile exists because the good tools in this category are paid and closed. The GPL keeps it that way round: anyone may use, study, modify and share it, but a distributed derivative has to ship its source under the same terms. Nobody gets to close it and sell it back to you.

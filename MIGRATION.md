# Rust + GPUI migration

## Result

TraceBit 0.3.0 is now a native Rust + GPUI Windows application. The old Electron main process, renderer HTML/CSS/JavaScript, Node package files and PowerShell poller have been removed.

## Preserved behavior

- Existing `%APPDATA%/TraceBit` directory and camelCase `settings.json` contract.
- JSONL daily events, JPEG snapshots and JSON reports remain readable.
- Foreground application/title, idle attribution, lock/sleep pauses and sensitive-window screenshot suppression.
- Opt-in browser hostname recording through native UI Automation.
- Three-stage screenshot deduplication and retention/cap maintenance.
- Redacted OpenAI-compatible reports, evidence-based resource recommendations and optional abstract-only web searches.
- Tray, autostart, automatic report notifications and single-instance execution.

## Native replacements

| Old | New |
| --- | --- |
| Electron renderer | GPUI entities and elements |
| Electron IPC | Direct shared Rust state |
| PowerShell poller | Rust Win32/UI Automation sampler thread |
| Electron desktopCapturer | Native screen capture + Rust image processing |
| Node fetch | reqwest blocking client on background threads |
| Electron tray/login items | tray-icon + Windows registry |
| NSIS via electron-builder | `installer.nsi` + `scripts/dist.cmd` |
| External window icon | Embedded ICO + `WM_SETICON` |

## Verification

- `cargo test`: 17 tests passed, including an end-to-end mock AI report with redaction and persistence.
- `cargo run -- --native-smoke`: passed with real foreground application events and a JPEG screenshot.
- `cargo build --release`: passed.
- NSIS installer generated at `dist/TraceBit Setup 0.3.0.exe` (about 4.4 MB, compared with about 84 MB for the previous Electron installer).
- GPUI window, history data, two-column layout, settings and window icon were checked on Windows.
- Text input focus, clipping, masked rendering and horizontal scrolling were corrected; character entry is being manually verified by the user.

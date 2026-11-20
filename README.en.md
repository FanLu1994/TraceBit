# TraceBit

[简体中文](README.md) · [English](README.en.md) · [日本語](README.ja.md) · [한국어](README.ko.md)

[![CI](https://github.com/FanLu1994/TraceBit/actions/workflows/ci.yml/badge.svg)](https://github.com/FanLu1994/TraceBit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/FanLu1994/TraceBit)](https://github.com/FanLu1994/TraceBit/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4)

An independent, local-first Windows app that records your activity and suggests AI-powered tool upgrades.

> Inspired by Alma Recorder.

TraceBit records foreground apps, window titles, idle/locked state and deduplicated screen snapshots as activity segments, turns each day's recording into numbered evidence on your machine, and keeps learning how you work: the tools you use most, the routes you run back and forth, your output rhythm, and the projects and sites you keep returning to. The AI is not there to correct your habits; it looks at what you already do and recommends tools, plugins, settings and open-source alternatives that save you steps.

The current version is a fully native **Rust + GPUI** app: the interface, capture, storage, AI requests and system integration are all implemented in Rust, with no need for Node.js, Electron, WebView, HTML/CSS or PowerShell.

## Installation

Windows 10/11 is required; no Rust or other runtime is needed.

Download the latest installer, **[TraceBit Setup (Releases)](https://github.com/FanLu1994/TraceBit/releases/latest)**, and run it:

- Installs to `%LOCALAPPDATA%\Programs\TraceBit` and creates Start menu and desktop shortcuts
- Installing over an existing copy keeps the recordings in `%APPDATA%\TraceBit`
- Uninstalling does not delete your personal records; clean up `%APPDATA%\TraceBit` manually if you want them gone

## Usage

The left navigation has four pages, switchable with shortcuts:

| Shortcut | Page | Contents |
| --- | --- | --- |
| `Ctrl+1` | Today | The day's timeline, app ranking, category share, snapshot review |
| `Ctrl+4` | Recent stats | 7 / 14 / 30-day summaries, daily curves, active hours, daily breakdown |
| `Ctrl+2` | AI advice | The day's conclusions, tool-upgrade candidates, opening the full HTML report |
| `Ctrl+3` | Settings | Recording, screenshots, sites, AI, daily digest and system integration |

Other shortcuts: `Ctrl+Shift+R` starts/pauses recording, `Ctrl+W` closes the window, `Esc` closes an overlay, and `Tab` / `Shift+Tab` moves focus.

### Configuring AI

Open “Settings → AI analysis” and fill in:

- **API base URL**: for example `https://api.deepseek.com/v1`, or `http://localhost:11434/v1` for a local Ollama
- **API key**: may be left empty for a local Ollama; stored encrypted with Windows DPAPI
- **Model name**: a multimodal model with image input is recommended, for example `qwen2.5vl` (local Ollama) or `gpt-4o-mini`; for text-only advice, pure text models such as `deepseek-chat` or `qwen-plus` work too

The base URL is used exactly as entered; only a bare domain (such as `http://localhost:11434`) gets `/v1` appended automatically, so paths like `/v1beta/openai` and `/api/paas/v4` can be entered as-is. Requests use JSON output mode (falling back automatically when the service does not support it), retry on 429/5xx, and fail with a clear error when the output is truncated.

Once saved, you can run “Test connection”, then pick a date on “AI advice” and generate. Before generating, click “View sent content” to check the entire text that will be sent. With “Daily auto-generate” enabled, the day's advice is generated at the scheduled time and delivered as a Windows notification; if the computer was asleep or the app was not running, or if there was a lot of activity after generation, it is generated the next day as a catch-up, without overwriting advice you generated manually.

The work profile is backfilled over the last 30 days at startup, then refreshed once a day every 10 minutes, whether or not AI is configured. It is a local JSON file that you can view or delete at any time; delete it and it is learned again on the next start.

## Features

### Recording

- Records foreground apps and window titles, automatically categorized as development, browser, communication, meetings, documents and more
- Splits activity segments using Windows' real keyboard/mouse idle time; uses a longer idle threshold during meetings, full-screen video or presentations; pauses capture while the screen is locked or the computer is asleep
- Filters transient windows from Alt-Tab passes, accumulates time by window title and site; activity spanning midnight is not lost
- Takes snapshots on app switches and on a timed heartbeat (capturing the monitor that hosts the foreground window); three-level deduplication by thumbnail hash, histogram and pixel difference
- Optional site recording: when you turn it on, Windows UI Automation is used to save only the current browser domain, and browser time is assigned to development, documents, entertainment and other categories by site
- Runs as a single instance, with the window and the tray icon embedded in the executable, so there is no extra resource directory

### UI and stats

- Native GPUI interface: Today, a 24-hour timeline, category share, app ranking, snapshot review
- UI languages: Simplified Chinese, English, 日本語, 한국어; follows the Windows display language by default, and a change takes effect as soon as you save it
- Recent stats: 7 / 14 / 30-day overview with a per-day activity curve, active hours, top apps and category share, frequent sites, and a clickable daily breakdown
- Click any row in the daily breakdown to jump straight to Today for that day
- System tray, close to tray, launch at startup, capacity rotation and one-click clear

### Work profile and AI advice

- Work profile: merges every recording day into a long-term profile (main tools and what they are used for, daily routes, active rhythm, recurring projects and sites), which serves as the long-term basis for AI advice
- AI tool upgrades: evidence and the long-term profile are computed locally, and the AI only has to understand how you work and then propose upgrades that land on a specific tool, plugin, shortcut, setting or automation; every item must cite evidence numbers, and numbers that cannot be verified are flagged
- Tool upgrades only, no forced habit change: anything asking you to do less, quit, batch tasks at fixed times or go to bed earlier is filtered out by local rules, and reports keep only suggestions that make your existing actions take fewer steps
- Open-source alternatives: local rules detect commercial tools you have used for a long time (API clients, database clients, design/editing software, remote control, compression tools and so on) and suggest open-source alternatives that work in a similar way; the AI can only reference them by number
- Adoption loop: mark items “Adopted / Not needed” in the app; dismissed candidates are no longer recommended, and adopted tools are automatically compared against changes in similar routes in the next report
- Daily digest: by default at 10:30, a system notification delivers yesterday's conclusions plus a short greeting; it stays quiet when yesterday has no recording, and can be turned off or rescheduled; a missed delivery is caught up once on the same day
- The full report opens as a standalone HTML page in your browser: work profile, how you work, tool-upgrade opportunities, the next automatic check, adoption review and other candidate tools, with clickable evidence numbers, dark mode and printing support
- Observes, never assigns homework: the report notes a few metrics worth comparing next time and checks them automatically, with no experiment for you to run
- Optional screenshot interval summaries: snapshots are picked in hour-aligned intervals along with the foreground app at the time and summarized by the multimodal model configured above, and those summaries become evidence for advice
- Toolable-operation recommendations: local rules match open-source projects, Skills and MCP servers against real switching and duration evidence, then hand them to the AI as candidates
- Optional online resource search: supports DuckDuckGo and Brave Search, and keeps locally curated recommendations when a search fails
- Works with OpenAI-compatible APIs: DeepSeek, Qwen (Tongyi), Kimi, OpenAI, local Ollama and more
- Generates advice automatically (missed runs are caught up, and manual results are never overwritten)

## Data and privacy

By default, data lives here:

```text
%APPDATA%\TraceBit\
├── settings.json             # Settings; the API key is encrypted with DPAPI
├── profile.json              # Locally learned work profile (one entry per day) and tool decisions
├── digest.json               # Daily digest state (date of the most recent delivery)
├── data\
│   ├── days\YYYY-MM-DD.jsonl # Foreground, idle, lock and snapshot index events
│   └── snapshots\YYYY-MM-DD\ # JPG snapshots
├── reports\YYYY-MM-DD.json   # Generated advice (evidence, work profile, tool decisions and token usage)
├── reports\YYYY-MM-DD.html   # Browser version of the same advice report
└── intervals\
    ├── YYYY-MM-DD.jsonl      # Screenshot interval summaries
    ├── last_request.json     # Text and screenshot list last sent to the vision model
    └── cursor.json           # Interval analysis progress
```

The Rust version keeps these formats and can read history from older versions directly. Newly written events carry a `v` version number, and the old format is still readable.

- Activity records, snapshots and summaries stay on this machine; keystrokes are never recorded. With a remote AI, only the content listed below is sent.
- Site recording is off by default. When you enable it, only the address bar of supported browsers is read, through Windows UI Automation; site records keep the domain alone, with no paths, query parameters or page content. Screen snapshots are controlled by a separate switch.
- Nothing is written and no screenshot is taken while the screen is locked or the computer is asleep.
- Foreground windows matching sensitive keywords (judged by app name, title and site): the title is stored on disk as `[敏感窗口]`, no screenshot is taken and no site is recorded; you can add or remove keywords in Settings.
- AI advice sends only de-identified evidence computed locally: durations, counts, categories, app names, truncated window titles, and the locally aggregated long-term work profile (tool names, app pairs in routes, rhythm, recurring window titles); chat apps contribute only their app name, never conversation titles; **neither the screenshots themselves nor the visited site domains are ever sent to the advice model** — the site list appears only in the local report. Screenshot interval summaries (if enabled) are sent as text evidence.
- Screenshot interval analysis is off by default. When enabled, snapshots are sent to the model API configured above (local or cloud); the image may contain window titles, site addresses, chat content or other sensitive information, so use a local multimodal model when privacy matters.
- Online resource search is off by default. When enabled, the search service receives only the built-in abstract tool keywords, never window titles, site domains, URLs, screenshots or usage statistics; source-link validation only visits public resource URLs.
- Before sending, text is masked for emails, keys and tokens, bank card numbers, ID numbers, phone numbers, IPs, URLs with parameters, and `C:\Users\<username>` paths. Use a local model when privacy matters.
- Snapshots capture the monitor that hosts the foreground window. Sensitive keywords are judged from the foreground app, title and site, so sensitive content in background windows cannot be detected.

## Development

Install stable Rust (MSVC toolchain) and the Visual Studio 2022 C++ Build Tools, then:

```bash
cargo run
```

You can also double-click `start.cmd`.

### Self-test

```bash
cargo test                  # Settings, storage, stats, redaction, recommendations and the full simulated AI pipeline
cargo run -- --native-smoke # Real foreground capture and JPEG snapshots
```

You can also run `scripts\smoke.cmd`. The native smoke run works in a temporary directory and deletes its test records and snapshots when it finishes, so your real data is left untouched.

### Packaging

Install [NSIS 3](https://nsis.sourceforge.io/), then run:

```bat
scripts\dist.cmd
```

The script runs `cargo build --release` and produces `dist\TraceBit Setup <version>.exe`. The shipped program is a single `tracebit.exe` with an embedded icon — no Electron runtime and no PowerShell scripts.

Pushing a `v*` tag makes `.github/workflows/release.yml` build the installer and upload it to the matching Release; commits and PRs run fmt, clippy, test and a release build through `.github/workflows/ci.yml`.

### Project layout

```text
src/
  main.rs            # Entry point and smoke commands
  app.rs             # GPUI window, four-page UI, tray and background tasks
  input.rs           # Native GPUI text input widget
  platform.rs        # Win32, UIA, window icons, autostart and notifications
  i18n.rs            # Simplified Chinese / English / 日本語 / 한국어 text selection and language detection
  recorder.rs        # Capture thread, snapshot deduplication and privacy rules
  store.rs           # JSONL daily storage and timeline/segment/app/site/category stats
  stats.rs           # Rolling-window aggregation for the recent period
  insights.rs        # Local metrics and numbered evidence (deep work, interruptions, fragmentation, trends)
  profile.rs         # Long-term work profile: tools, routes, rhythm, recurring projects, and tool decisions
  analyzer.rs        # Advice generation: prompt, JSON validation, work profile, capability-boundary filtering, Markdown rendering
  report_html.rs     # Standalone HTML report for advice
  llm.rs             # OpenAI-compatible calls: JSON mode, retries, truncation detection
  redact.rs          # Pre-send redaction and title cleanup
  secret.rs          # DPAPI key encryption
  intervals.rs       # Screenshot interval analysis, multimodal requests, image encoding and summary storage
  recommendations.rs # Local operation detection and curated resources (tool candidates for the AI)
  web_search.rs      # Optional abstract search, filtering and source validation
  categories.rs      # App and site categories, meeting detection
  settings.rs        # Local settings with legacy camelCase compatibility
assets/              # Icons embedded at build time
build.rs             # Windows exe icon resource
installer.nsi        # NSIS installer definition
scripts/dist.cmd     # Release and installer build
scripts/smoke.cmd    # Unit tests plus real-capture smoke
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for environment requirements, pre-commit checks, code conventions and the release process. Include verification results with every change, and screenshots for UI changes.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

[MIT](LICENSE)

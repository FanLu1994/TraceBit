# TraceBit UI direction

## 1. Visual theme
A light, neutral workbench for reviewing a day: gray canvas, dark ink, one amber trace as the brand anchor. Dense enough for daily utility, never a wall of interchangeable cards. Chinese UI copy states the data, state, or next action directly.

## 2. Color roles
- Canvas: `#f3f5f6`
- Surface: `#ffffff`
- Surface subtle: `#e9edf0`
- Ink: `#202428`
- Secondary ink: `#565e65`
- Faint ink: `#747c83`
- Divider: `#d7dde1`
- Brand amber: `#c88918`
- Brand pale: `#f6ecd7`
- Active green: `#2f7d57`
- Danger red: `#b64b4b`
- Category colors stay reserved for data, never decoration.

## 3. Typography
- UI: `Segoe UI Variable`, `Microsoft YaHei UI`, `PingFang SC`, sans-serif
- Numbers: same UI family with tabular figures, not a separate terminal face
- Page title: 24px / 650
- Section title: 18px / 650
- Body: 14px / 1.7 for Chinese
- Caption: 12px / 1.5, never below 12px
- No negative tracking on Chinese text.

## 4. Components
- One global recording control with an unambiguous verb: `开始记录` or `暂停记录`.
- Primary buttons: ink background, white text, 7px radius. Secondary: transparent with divider.
- Status summary is one open strip with generous whitespace and a bottom divider, not separate metric cards.
- Navigation uses text and consistent 18px line icons. Selected state is a pale amber wash.
- Inputs have visible labels and inline action feedback. Settings save is sticky and explicit.
- Interval summaries are a separate Today section, newest first, with time range, image count, plain-text summary, and inline failure feedback.

## 5. Layout
- 184px navigation rail, flexible workspace.
- 28px page gutter, 20px section spacing.
- Today: open status strip, unframed 24-hour timeline, two-column application/category area, optional interval summaries, sites, and snapshots. Use panels only for empty states, self-contained forms, and report documents.
- Collapse the two-column area below 1100px.
- Minimum supported window: 960×620. Every target remains at least 40px high.

## 6. Depth
- Canvas and surface differ by neutral luminance steps.
- Data sections use spacing and dividers instead of matching card containers. Borders are 1px and neutral. Shadows are reserved for the lightbox and toasts.
- Radius scale: 5px controls, 9px panels, 14px lightbox.

## 7. Do and do not
- Do put recording state and its action in the first viewport.
- Do use real empty states with one next action.
- Do show loading, failure, paused and active states explicitly.
- Do not use dark card grids, gradient text, glass, decorative charts or vague icon-only primary actions.
- Do not wrap every data region in an identical rounded card.
- Do not use tinted success/error boxes or slogans as section subtitles.
- Do not make the user save a form just to start or pause recording.
- Keep daily reports text-only. Screenshot interval analysis is a separate opt-in requiring a vision-capable model; never upload a historical backlog after first enablement.
- Limit interval analysis to 6 evenly spaced snapshots, downscaled to a 1024px long edge and re-encoded as JPEG.

## 8. Native Windows behavior
- Keep the Rust/GPUI native stack. Use the real title bar, Windows notifications, and an owner-bound native warning dialog for destructive actions.
- Use the arrow cursor on buttons, navigation, list rows, and thumbnails. Reserve the hand cursor for content links and the I-beam for editable text.
- Tab / Shift-Tab moves through controls; Enter / Space activates the focused control. Focus uses a visible amber outline without changing layout.
- Escape closes the snapshot viewer or dismisses inline feedback.
- Shortcuts: Ctrl+1 Today, Ctrl+2 Report, Ctrl+3 or Ctrl+, Settings, Ctrl+Shift+R toggle recording, Ctrl+W close window.
- A second launch restores and focuses the existing window.
- The snapshot viewer has a header, a viewport-bounded image area, and a footer toolbar. It must not overflow at the minimum window size.

## 9. Motion
- Press feedback: scale to 0.98 in 120ms.
- Hover color changes: 120ms.
- Toasts: opacity and small translate, 160ms.
- No layout animation and no `transition: all`.
- Respect `prefers-reduced-motion`.

## 10. Verification
- Check 1240×840 and 960×620 in the native GPUI window.
- Check today with sparse data, empty day, recording active and paused.
- Check report unconfigured, generating, generated and error states.
- Check interval analysis opt-in, first-interval waiting, sparse screenshots, vision-model errors, cross-midnight intervals, and summary persistence.
- Check settings save and global toggle remain synchronized.
- Check long window titles and long input values do not overflow their containers.
- Check titlebar, taskbar and tray icons, plus close-to-tray restoration.
- Check the snapshot viewer at both window sizes, including long metadata, first/last image, Escape, and keyboard activation.
- Check Tab / Shift-Tab, Enter / Space, global shortcuts, destructive confirmations, and second-launch window restoration.
- Run `cargo test` and `cargo run -- --native-smoke` after collector changes.

# TraceBit UI direction

## 1. Visual theme
A quiet, light workbench for reviewing a day: warm paper canvas, ink text, one amber trace as the brand anchor. Dense enough for daily utility, never a wall of interchangeable cards. Chinese UI copy is direct and task-led.

## 2. Color roles
- Canvas: `#f4f2ed`
- Surface: `#fffefb`
- Surface subtle: `#ebe8e1`
- Ink: `#24231f`
- Secondary ink: `#6f6b62`
- Faint ink: `#979186`
- Divider: `#dedad1`
- Brand amber: `#c88918`
- Brand pale: `#f4e7c8`
- Active green: `#2f7d57`
- Danger red: `#b64b4b`
- Category colors stay reserved for data, never decoration.

## 3. Typography
- UI: `Segoe UI Variable`, `Microsoft YaHei UI`, `PingFang SC`, sans-serif
- Numbers: same UI family with tabular figures, not a separate terminal face
- Page title: 24px / 650
- Section title: 15px / 650
- Body: 14px / 1.7 for Chinese
- Caption: 12px / 1.5, never below 12px
- No negative tracking on Chinese text.

## 4. Components
- One global recording control with an unambiguous verb: `开始记录` or `暂停记录`.
- Primary buttons: ink background, white text, 7px radius. Secondary: transparent with divider.
- Status summary is one divided strip, not six separate cards.
- Navigation uses text and consistent 18px line icons. Selected state is a pale amber wash.
- Inputs have visible labels and inline action feedback. Settings save is sticky and explicit.

## 5. Layout
- 184px navigation rail, flexible workspace.
- 28px page gutter, 20px section spacing.
- Today: status strip, 24-hour timeline, two-column application/category area, snapshots.
- Collapse the two-column area below 1100px.
- Minimum supported window: 960×620. Every target remains at least 40px high.

## 6. Depth
- Canvas and surface differ by warm luminance steps.
- Borders are 1px dividers. Shadows are reserved for the lightbox and toasts.
- Radius scale: 5px controls, 9px panels, 14px lightbox.

## 7. Do and do not
- Do put recording state and its action in the first viewport.
- Do use real empty states with one next action.
- Do show loading, failure, paused and active states explicitly.
- Do not use dark card grids, gradient text, glass, decorative charts or vague icon-only primary actions.
- Do not make the user save a form just to start or pause recording.

## 8. Motion
- Press feedback: scale to 0.98 in 120ms.
- Hover color changes: 120ms.
- Toasts: opacity and small translate, 160ms.
- No layout animation and no `transition: all`.
- Respect `prefers-reduced-motion`.

## 9. Verification
- Check 1240×840 and 960×620 in the native GPUI window.
- Check today with sparse data, empty day, recording active and paused.
- Check report unconfigured, generating, generated and error states.
- Check settings save and global toggle remain synchronized.
- Check long window titles and long input values do not overflow their containers.
- Check titlebar, taskbar and tray icons, plus close-to-tray restoration.
- Run `cargo test` and `cargo run -- --native-smoke` after collector changes.

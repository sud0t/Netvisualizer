# Building the Terminal UI

A working brief for whoever (person or agent) builds the viewer's interface
out in the **Terminal** direction: what is decided, what already exists,
what every region must look like and do, and the traps found while the
first version was built. The concise project spec is `project_spec.md`;
this file is the detailed companion for the UI only.

Status at the time of writing (2026-10-23): the interface was rebuilt on
`bevy_lunex` in the *Radar* direction (round Scan button, chips on an arc)
and Kyler chose *Terminal* from the four directions on the design page
(https://claude.ai/artifact/DHP5jwzAN6xztMAdj28axX). The theme, the
shader, the widgets and the text fields have been moved to Terminal; the
HUD, the dock and the rail still have the Radar layout and are the work
in front of you (see "Work remaining").

## Ground rules

These come from Kyler and apply to every change:

- **Nothing touches the network without a user action.** Only `--scan` on
  the command line or the Scan control starts a scan. No scan on start-up,
  no probe behind a toggle, no "refresh".
- **Minimal dependencies.** Add a crate only when the program uses it. A
  new dependency means Kyler runs `cargo agents sync` afterwards; say so in
  the report. Use `cargo agents crate-info <crate>` to read a crate's
  source before guessing at its API.
- **Comments explain.** This is a learning project: a comment says why a
  thing is the way it is, in plain words, not what the next line does.
- **`docs/project_spec.md` stays concise** (about a hundred lines). The
  "Feature requests" header in it is Kyler's; only delete items from it
  when they are done.
- **Never ask about committing.** Write a commit message at the end of a
  milestone; Kyler commits.
- Lints are pedantic and warnings are errors (`[workspace.lints.clippy]`
  in `Cargo.toml`); `type_complexity` is allowed workspace-wide.

## The stack

| Piece | Version | Notes |
| --- | --- | --- |
| Bevy | 0.20 (edition 2024, rustc 1.99.1 pinned) | `Text2d`, `bevy_picking`, `Material2d` |
| bevy_lunex | 0.8.0, git `bytestring-net/bevy_lunex` rev `15fdc857d938a7aba138f07424a98890245e9779` | The "update to bevy 0.20" commit on main; crates.io only has 0.7 (Bevy 0.19). `default-features = false` (no text3d). |
| arboard | 3.7, `default-features = false`, `wayland-data-control` | Ctrl+C / Ctrl+V in the text fields |
| JetBrains Mono | variable TTF, OFL, `viewer/assets/fonts/` | Embedded with `include_bytes!`; the only face. Inter was removed with the Terminal look. |

Modules under `viewer/src/`:

| File | Holds |
| --- | --- |
| `theme.rs` | Every colour, size and look. The one place a palette changes. |
| `scanform.rs` | `ScanForm`: the scan's toggles (`Feature::{Arp,Trace,Names,Tcp,Udp}`), network, interface cycler, timeout and concurrency presets, remembered port lists; `settings()` turns it into `ScanSettings`. |
| `panel.rs` | The host list's model: `Panel {collapsed, filter, focus_filter, rows}`, `visible_indices()`, `sidebar_width()`, `frame_inset()` (what the UI hides, for the camera framing). |
| `ui/mod.rs` | `UiPlugin`, the UI camera and lunex root, z constants, `UiInput`, `Status`, `Help`, `UiRoot`, `UiHidden`, the scanlines box, `root_point`, `role_color`, `speed_text`. |
| `ui/shape.rs` + `ui/shape.wgsl` | `Look` → `Shape` uniform → `ShapeMaterial`; `Skin` quads that follow their node's size. |
| `ui/widgets.rs` | `Fonts`, `TextStyle`, `truncate`, `Pin`, `FitText`, `Styled` (hover blend), `Toggle`, `Tooltip`, `Kit`, `ButtonSpec`, `button()`, `toggle()`, `set_width`. |
| `ui/text_input.rs` | `TextField` (value, prompt, hint), `Focus`, `Submitted`, `FocusField`, `text_field()`. |
| `ui/hud.rs` | The status box top left and the legend under it. |
| `ui/dock.rs` | The scan dock bottom centre: Scan, the toggles, the fields, the options drawer. `DOCK_INSET` is read by `panel.rs`. |
| `ui/rail.rs` | The host table and the details dump down the right edge; virtual lists. |
| `ui/labels.rs` | The host names over the spheres. |
| `ui/help.rs` | The controls overlay. |
| `ui/tooltip.rs` | Tooltips after a 0.45 s rest. |
| `ui/pick.rs` | The picking backend: which nodes are under the pointer, UI camera only. |
| `startup.rs` | Start-up flow and the `--script` driver. |
| `cli.rs` | Flags, including `--no-ui` and `--script` under Debugging. |

## How the UI layer works

**Two cameras.** The 3D camera draws the map. A second camera (`UiCamera`,
`Camera2d`, `order: 1`, `ClearColorConfig::None`, `RenderLayers::layer(1)`)
draws the UI over it. It must be `Hdr` with `Tonemapping::None` and
`DebandDither::Disabled`, the same kind of target as the 3D camera:
cameras of different kinds get separate textures, and a texture that is
never cleared piles every frame's UI onto the last one until the panels
are opaque.

**One root.** `UiLayoutRoot::new_2d()` + `UiFetchFromCamera::<0>` sized to
the window, with `Propagate(RenderLayers::layer(1))` so every node under
it is on the UI layer. `ui/pick.rs` reports what the pointer is over
(lunex's own backend is silenced; see the traps). The root and every full-size or purely decorative
node carry `Pickable::IGNORE`; otherwise `UiInput.pointer` reads "over the
UI" everywhere and the 3D view never gets a click.

**Depth.** `UiDepth::Set(z)` on a region's top node; children stack a
level (1.0) above their parent. Constants in `ui/mod.rs`: labels 1,
panels 10, rail 15 (above the dock, which can reach under it in a narrow
window), help 100, tooltips 200, scanlines 300.

**A node is painted by a `Skin` child**, a shared unit quad with a
`ShapeMaterial`, scaled to the node's `Dimension` by `fit_skins` after
lunex's `UiSystems::PostCompute`. The quad is `size + 2 × pad`; `pad` is
room for a glow, so a look with a glow needs a skin spawned with that
pad (`kit.skin(theme::panel(), theme::PANEL_GLOW)`), or the glow is
clipped. The shader (`shape.wgsl`) paints, in order: glow outside the
edge, fill, border band inside the edge, ring and arc (circles), then the
scanlines (every third physical row darkened by `scanline`). `Look::lerp`
blends two looks for hover transitions.

**Text is plain `Text2d`**, never lunex's text sizing (lunex measures in
physical pixels, wrong on HiDPI). A text child carries a `Pin`
(left/centre/right, pad, dy) and `pin_texts` places it from its node's
width; a node that should be as wide as its text carries `FitText`, and
`fit_text_nodes` (in `UiSystems::PreCompute`) sets the layout width from
`TextLayoutInfo.size / scale_factor`. `TextStyle::width_of` and
`truncate` estimate widths before layout (0.6 em a character).

**Widgets.** `button(ui, kit, ButtonSpec, on_click)` spawns a node with
`hoverable()` (lunex `UiHover`), a pointer cursor, a skin, a centred text,
and a `Styled {skin, base, hover, text}`; `blend_styles` paints between
the two looks by the hover weight. `toggle(...)` is a button plus a
`Toggle` whose `sync_toggles` swaps looks *and label* (`[x] ARP` /
`[ ] ARP`) when `on` changes. Observers take `On<PointerClick>` and check
`click.button == PointerButton::Primary`; the parameter type for a passed
observer is `impl IntoEntityObserver<M>`.

**Text fields.** `text_field(ui, kit, name, layout, prompt, value, hint,
style)`. One field has `Focus` at a time; Enter sends `Submitted(entity)`;
Escape or a click elsewhere blurs; Ctrl+V/C use arboard; the caret is a
block the width of a character. While a field has focus
`UiInput.keyboard` is true and the fly keys are off. The field's value is
the source of truth for what is shown; the owning panel copies it into
the model (`read_fields`, `read_filter`) and copies the model back
(`show_form`) so both directions stay in step.

**Virtual lists.** `VirtualList {row_h, offset, total, slots}` in
`rail.rs`: `size_lists` keeps as many row slots as fit in the node's
height, `scroll_list` (a `PointerScroll` observer) moves the offset, and
the fill systems write the visible window into the slots. Nothing is
clipped, so a list node must not be taller than its rows.

**Input flags.** `read_ui_input` (first in `Update`) sets `UiInput
{pointer, keyboard}` from the `HoverMap` (filtered to nodes with a
`Dimension`) and `Focus`; a press that began on the UI keeps `pointer`
true until release, so a drag off a panel never turns the camera.

**Messages the UI sends:** `StartScan` (the same one `--scan` sends;
`scan.rs` does the rest), `SelectHost {id, fly}`, `ResetView {animate}`,
`FocusField`, `Submitted`. The map is `MapDocument`; `doc.0 = None` is
Clear; `map.forget(id)` drops one host. `ScanJob` exists while a scan runs
(`job.stage`, `job.progress: Option<(done, total)>`, `job.cancel()`).

## The Terminal look

Everything is monospace text in near-black boxes with hard corners, lit in
phosphor green, with scanlines over the whole window. The role colours
stay as they were so the table, the legend and the spheres agree.

### Tokens (`theme.rs`)

| Token | Value | Use |
| --- | --- | --- |
| `ACCENT` / `ACCENT_TEXT` | `#39ff88` | The phosphor: brand, lit toggles, Scan, focused borders |
| `TEXT` | `#9ff5c0` | Ordinary text |
| `MUTED` | `#4f8f66` | Keys, hints, idle controls, counts |
| `ERROR` / `STOP` | `#ff6b6b` | Errors; the Stop state |
| `HUD_BG` | `#030605` at 90 % | Panels and fields |
| `HUD_BG_SOLID` | `#030605` at 95 % | Cards, tooltips, the help card, the selected label |
| `HUD_BORDER` | phosphor 42 % | Every hairline border |
| `HUD_BORDER_HOVER` | phosphor 75 % | Under the pointer |
| `HUD_BORDER_SOFT` | phosphor 30 % | The rule under a header band |
| `BAND` | phosphor 7 % | A header band's fill |
| `ROW_HOVER` | phosphor 8 % | Hovered rows and ghost buttons |
| `ROW_SELECTED` / `ROW_SELECTED_BORDER` | phosphor 14 % / 50 % | The selected host's row |
| `ACCENT_FILL` / `ACCENT_FILL_HOVER` | phosphor 12 % / 22 % | Toggle and Scan fills |
| `ACCENT_GLOW` / `PANEL_GLOW_COLOR` | phosphor 18 % / 4 % | Glows (toggle under the pointer, Scan; panels). Kept faint on purpose: Kyler asked for the bloom toned down. |
| `STOP_FILL` / `STOP_FILL_HOVER` / `STOP_GLOW` | red 12 % / 22 % / 18 % | Scan while running |
| `BACKDROP` | black 60 % | Behind the help card |
| `ROLE_SCANNER` cyan `#22d3ee`, `ROLE_GATEWAY` amber `#f59e0b`, `ROLE_ROUTER` violet `#c084fc`, `ROLE_HOST` blue `#60a5fa` | unchanged | Roles; `ROLE_DOWN` is a dull green-grey `#5b6f63` |
| `SCENE_BG` | `#040705` | The clear colour, the planet's surface under the sky, the fog |
| `GRID_LINE` / `GRID_AXIS` | `#0f3d24` / `#1c7a46` at `GRID_OPACITY` 0.6 | The grid floor |
| `LINK` | `#2f945a` at `LINK_OPACITY` 0.55 | Links between hosts |
| `SELECTION` | phosphor | The ring round the selected sphere |

Sizes: `EDGE` 10 (panels from the window edges), all radii 0,
`PANEL_GLOW` 8 px, `CHIP_GLOW` 4 px, `CHIP_PAD_X` 8 px,
`SCANLINE_STRENGTH` 0.12 (period 3 physical pixels, in the shader).

Looks: `panel()` (fill, hairline, glow; spawn its skin with
`PANEL_GLOW` pad), `card()` (solid, no glow), `band()` (header band fill;
add a 1 px `HUD_BORDER_SOFT` rule node under it), `chip_on()` /
`chip_on_hover()` / `chip_off()` / `chip_off_hover()`.

### Type scale (`TextStyle`)

| Style | Face | Size | Use |
| --- | --- | --- | --- |
| `BODY` | regular | 13 | Buttons, fields, the status line |
| `TITLE` | bold | 13 | Panel titles, in capitals written by the caller |
| `LABEL` | bold | 13 | The toggles |
| `MONO` | regular | 12 | The host table, the details dump, keys, hints |
| `SMALL` | regular | 11 | Small print |

There is no letter-spacing or text-transform in Bevy text: write capitals
in the string. Bold comes from the variable font's weight axis
(`FontWeight::BOLD`).

### Conventions of the look

- Square brackets mark anything pressable: toggles `[x] ARP` / `[ ] ARP`,
  the Scan block `[ SCAN ]` (`[ STOP ]` while running), small buttons
  `[x]`, `[fly to]`, `[forget]`, `[>]`, key hints `[?] help`, `[/] filter`,
  `[R] reset`.
- Fields have a prompt: `net> 192.168.1.0/24▮`, `if> auto`, `filter> `.
- A header band is a 30 px strip across the top of a panel: `BAND` fill,
  a 1 px rule under it, the title in `TITLE` `ACCENT` capitals, counts in
  `MUTED`.
- Lit (`ACCENT`) means on or primary; `TEXT` means content; `MUTED` means
  a key, a hint or an idle control. Hover brightens the border, never the
  fill alone.
- Role words and swatches are in the role colour; nothing else borrows
  those colours.
- Progress is text: a stage name, `done/total`, and a 16-cell bar of `█`
  and `░` (fall back to `#` and `.` if the glyphs do not render).

## Layout, region by region

All numbers are logical pixels at the 1280 × 800 reference window. The
rail is `sidebar_width()` wide: **330** from here on (220 in windows up to
720 wide); change `SIDEBAR_WIDTH` in `panel.rs`.

### HUD (top left, `hud.rs`)

```
┌──────────────────────────────────────────────────────────┐
│ NETVISUALIZER v0.1 // demo 192.168.1.0/24 · 27 hosts · … │
│ fly 18 u/s   [?] help   [/] filter   [R] reset            │
└══════════════════════════════════════════════════════════┘   ← progress line
```

- One `panel()` box at (`EDGE`, `EDGE`), hugging its content: a column
  flow (padding 12 × 8, gap 4) of two row flows (height 20, gap 8).
- Row 1: brand `NETVISUALIZER` (`TITLE`, `ACCENT`), the version
  (`env!("CARGO_PKG_VERSION")`, `MONO`, `MUTED`), `//` (`MUTED`), the
  status text (`MONO`; `TEXT`, or `ERROR` while `Status.error`). The
  status is truncated to the room between the brand and the rail
  (`show_status`: `STATUS_MIN_PX` 120, `STATUS_MAX_PX` 480, the fixed part
  of the row about 200).
- Row 2: `fly 18 u/s` (`MONO`, `MUTED`, tooltip "fly speed: the mouse
  wheel changes it"), then three ghost buttons (`MONO`, `MUTED`, `ACCENT`
  under the pointer, `FitText` pad 4): `[?] help` toggles `Help.open`,
  `[/] filter` calls `panel.focus_filter()`, `[R] reset` sends
  `ResetView {animate: true}`.
- The progress line: a 2 px `ACCENT` line along the box's bottom edge
  from the left, as long as the running stage's fraction; alpha 0 when
  no scan runs (`show_progress`).

### Legend (under the HUD)

A second `panel()` box 8 px under the HUD, height 28, padding 12, gap 16:
for each of scanner, gateway, router, host an 8 × 8 square swatch
(`Look::filled(role_color, 0)`) and the name in `MONO` in the role colour.

### Dock (bottom centre, `dock.rs`)

```
          [x] ARP    [x] TRACE    [x] NAMES    [ ] TCP    [ ] UDP

 ┌───────────────────────┐  ╔══════════════╗  ┌──────────┐ ┌──────┐ ┌───────┐
 │ net> 192.168.1.0/24▮  │  ║   [ SCAN ]   ║  │ if> auto │ │ opts │ │ clear │
 └───────────────────────┘  ╚══════════════╝  └──────────┘ └──────┘ └───────┘
```

- The dock is an invisible window node (`Pickable::IGNORE`) `DOCK_W` 940
  wide, bottom-centred `EDGE` above the window's bottom, and shifted left
  by `dock_shift()` = half of (rail width + `EDGE`) while the rail is open,
  so Scan is centred in the free part of the window. `center_dock`
  re-centres it when `Panel.collapsed` changes.
- `BUTTON_CENTER` is the Scan block's centre: (`DOCK_W / 2`, `DOCK_H −
  24 − 2`). The block is 200 × 48: 2 px `ACCENT` border, `ACCENT_FILL`,
  `ACCENT_GLOW` glow 10 px (16 under the pointer), label `[ SCAN ]` in
  bold 18 px `ACCENT`. Tooltip: "find the hosts; nothing touches the
  network before this". Click: `scan_pressed` (Stop cancels the job;
  otherwise `start_scan` validates the form, shows an error in the
  status and the field, or sends `StartScan`).
- The toggle row sits 22 px above the block: five toggles centred on the
  block, centres 106 px apart, each `FitText` (so `[x] TRACE` is 86 px
  wide), height 24, `LABEL` style, labels in capitals from
  `Feature::label().to_uppercase()`, tooltip `Feature::describe()`. Click:
  `form.toggle(feature)`. `show_form` writes `form.enabled(feature)` back
  into each `Toggle.on`.
- Left of the block, 14 px away: the network field, 250 × 34, prompt
  `net>`, hint "the interface's subnet", `MONO`; Enter is Scan
  (`submit_network`). Right of the block, 14 px away, 8 px apart: the
  interface cycler 92 × 34 (`if>` prompt in `MUTED`, the label from
  `form.interface_label()`, click `form.cycle_interface()`), `opts` 60 ×
  34 (ghost, toggles `DockState.drawer_open`), `clear` 64 × 34 (ghost;
  shown only while the map has hosts and no scan runs; `clear_pressed`
  empties the map and deselects). These three carry `IdleOnly`.
- **Running:** the block reads `[ STOP ]` with `STOP` border, `STOP_FILL`
  and `STOP_GLOW`; the `IdleOnly` items hide and the stage text takes
  their place: `arp  12/254  ████████░░░░░░░░` (`MONO`, `TEXT`, 300 px,
  truncated), or the stage name alone while a stage reports no steps. The
  drawer hides while a scan runs.
- **The options drawer** opens above the toggle row, centred on the
  block, 318 wide: four rows of 32 (`timeout` and `at once` as `[-]
  value [+]` steppers over `TIMEOUT_STEPS` / `CONCURRENCY_STEPS`; `tcp
  ports` and `udp ports` as fields with hints "off; common or
  22,80,8000-8010"). `UiDepth::Add(2)` so it draws over anything it
  meets. Typing a port list turns its toggle on; turning a toggle off
  blanks the list and remembers it (`scanform.rs`).
- `DOCK_H` must hold the strip, the toggle row and the open drawer (about
  270); `DOCK_INSET` (what the dock hides for the camera framing) is the
  strip plus the toggle row plus `EDGE`, about 120.

### Rail (right edge, `rail.rs`)

```
┌───────────────────────────────────────────────┐
│ HOSTS 27/27          filter> ▮           [>]  │  ← band, 30 px
├───────────────────────────────────────────────┤
│ ROLE     HOST                 ADDR            │  ← MUTED header line
│ scanner  netvisualizer-host.  192.168.1.50    │
│▌gateway  gateway.lan          192.168.1.1    ▐│  ← selected: fill + outline
│ host     192.168.1.9                          │  ← label is the address: no ADDR
│ …                                             │
├───────────────────────────────────────────────┤
│ gateway.lan  gateway                     [x]  │  ← details band
├───────────────────────────────────────────────┤
│ ip        : 192.168.1.1                       │
│ hostname  : gateway.lan                       │
│ open_ports:                                   │
│   number  proto  service                      │
│   53      udp    —                            │
│ is_local  : —                                 │
│ [fly to]  [forget]                            │  ← footer, 26 px
└───────────────────────────────────────────────┘
```

- One `panel()` box from (`EDGE`, `EDGE`) to `EDGE` above the bottom,
  `RAIL_Z` = panels + 5. Inside, a column flow with no padding.
- **Head band** (`band()` + rule): `HOSTS` (`TITLE`, `ACCENT`), the
  count (`MONO`, `MUTED`; `27/27`, or `5/27` while filtered), the filter
  field growing to fill (prompt `filter>`, hint `(/)`, `MONO`), and `[>]`
  (ghost, collapses the rail). `/` anywhere focuses the field with its
  text cleared; Enter selects and flies to the first match.
- **Table header line** (20 px, `MONO`, `MUTED`): `ROLE` at x 8, `HOST`
  at x 68, `ADDR` at the address column.
- **Host rows** (`VirtualList`, `ROW_H` 21, `MONO`): the role word in
  `role_color(role)` at x 8; the label in `TEXT` at x 68, truncated to
  the room before the address column; the address in `TEXT` at
  `ADDR_COL` = rail width − 8 − 15 characters (left-aligned so the first
  digits line up; blank when the label already is the address; dropped
  altogether in the narrow rail). Hover `ROW_HOVER`; selected
  `ROW_SELECTED` with `ROW_SELECTED_BORDER`. Click selects and flies
  (`SelectHost {fly: true}`); the wheel scrolls two rows a notch.
  `keep_selection_in_view` scrolls to a selection made elsewhere.
- **Details** (hidden without a selection) at the bottom, at most
  `DETAILS_MAX_FRACTION` 0.55 of the rail: a band with the host's label
  (`TITLE`, `ACCENT`, truncated) and its role word in the role colour, and
  `[x]` at the right (deselect); a rule; the lines (`VirtualList`,
  `LINE_H` 20, `MONO`): `Line::Key` as `key padded to 10 + ":"` in
  `MUTED` with the value in `TEXT` at x 94, `Line::Head` as `key:` in
  `MUTED`, `Line::Mono` (table rows, already padded by `table_lines`) in
  `TEXT`; keys are the host JSON's own (`open_ports`, `is_local`), `ip`
  first; then a footer line with `[fly to]` and `[forget]` ghost buttons.
  Every field of the host JSON is rendered (scalars, lists of objects as
  tables, objects as nested keys), so new fields need no viewer change.
- **Collapsed:** the box hides and a `[<]` tab shows at the top right;
  the dock re-centres; `frame_inset().right` becomes 0.

### Help overlay (`help.rs`)

`BACKDROP` over the window (a click on it closes), a `card()` 460 wide in
the middle: `CONTROLS` title, twelve rows of key caps (`Look::bordered
(INPUT_BG, HUD_BORDER, 0)`, `MONO`) or a gesture in `MUTED`, and the
action in `TEXT`; footer "Press ? or Esc to close.". `HELP_ROWS` is the
table; add a row when a shortcut is added.

### Tooltips, labels

Tooltips: a `card()` with `MONO` `TEXT`, by the pointer (offset 14 × 20,
kept inside the window) after 0.45 s on a node with a `Tooltip` or any
descendant of one. Labels: a 0-radius box per sphere (`HUD_BG`,
`HUD_BORDER`; `HUD_BORDER_HOVER` hovered; `ACCENT` selected with
`HUD_BG_SOLID`), text in the role colour or `TEXT`, fading with the fog,
nearer ones drawn over farther ones, on the lowest layer so every panel
covers them.

## Behaviour checklist

Each of these worked in the Radar build and must still work after the
relayout; the scripted captures (below) are how to prove it.

- Every scan feature is a toggle on the dock; the TCP and UDP port lists
  follow their toggles both ways; timeout and concurrency step through
  presets; the interface cycles; the network field validates on Scan and
  shows the error in the status line and as a red border.
- Scan → `[ STOP ]` with the stage and progress; Stop cancels; the options
  and Clear hide meanwhile; the HUD progress line fills.
- Clear shows only with a map and no scan; forget drops one host.
- The filter narrows the table and the count; Enter selects the first
  match; `/` focuses it; Escape blurs; a click on the map blurs.
- Wheel scrolls both lists; a row click selects and flies; a sphere click
  or F scrolls the row into view; the details follow the selection.
- `[>]` collapses the rail to `[<]`; the dock re-centres; the camera
  framing follows `frame_inset`.
- `?`, `h` and `[?] help` open the overlay; Escape or the backdrop closes
  it; Escape with nothing open deselects.
- Tooltips on every control after 0.45 s; the pointer is a hand over
  controls and spheres, an I-beam over fields.
- `--no-ui` hides the whole layer; the status text truncates to the room
  left by the rail; nothing overlaps at 1280 × 800 or at 900 × 600.

## Traps found the first time

- **Lunex's own picking backend is off**: it takes the first active camera
  drawing to the window, and when the query hands it the 3D camera (not
  orthographic) it reports nothing, so the interface takes no clicks at
  all; which camera comes first changes with archetype order (a selection
  at start-up was enough). `ui/pick.rs` is our backend, UI camera only,
  and gives every node `NoLunexPicking`. Keep it.
- **A negative `Dimension` panics picking** (`Rect::from_center_size`):
  never size a node by subtraction that can go below zero (the HUD's
  progress line is a fraction of a track node for that reason).
- **A real mouse resting on the window** fights a `--script`: the driver
  drops the real devices' messages each frame while it runs, so keep that.
- **Lunex sizes text in physical pixels**: use `Text2d` + `Pin` +
  `FitText`, never `UiTextSize`.
- **Startup order**: resources the panels need (`Fonts`, `UiRoot`,
  `ShapeAssets`, `ScanForm`) are made in `PreStartup`; panels spawn in
  `Startup`. A "Resource does not exist" panic at start means something
  moved.
- **Query conflicts (B0001)**: two queries in one system that could both
  reach `Text2d` (or `Visibility`, `Dimension`) need `Without<Marker>`
  filters; `Single` queries on the camera need `Without<HostLabel>`.
  `Changed<VirtualList>` as a filter alongside `&mut VirtualList` is a
  conflict: use `list.is_changed()` on the `Mut` instead.
- **`UiState` and `CursorIconQueue` are not in the lunex prelude**; import
  them from `bevy_lunex` directly.
- **Observers**: events are `PointerClick`, `PointerOver`, `PointerOut`,
  `PointerScroll` (not `Pointer<Click>`); a passed-in observer's bound is
  `impl IntoEntityObserver<M>`.
- **Synthetic input** (the `--script` driver) must write the `WindowEvent`
  wrappers as well as `CursorMoved`, `MouseButtonInput`, `MouseWheel`
  (`phase: TouchPhase::Moved`) and `KeyboardInput`, or `bevy_picking`
  never sees it.
- **Flow layouts**: a flow without a width hugs its content
  (`UiFlowSize::Fit`); `Grow` takes the leftover; a flow row inside a
  window node sits at the node's top left, so a hugging row cannot be
  centred by anchor; use fixed positions (`at(center, size)`) when a row
  must be centred.
- **Glows need pad**: `kit.skin(look, pad)` with `pad` at least the glow
  radius.
- **A changed `Pin` or `Dimension`** is what re-places text; a system that
  moves text by editing its `Pin` relies on `pin_texts` reading
  `Ref<Pin>`.
- **The drawer and the rail** need their own depth (`UiDepth::Add(2)`,
  `RAIL_Z`) or they draw under what they overlap.
- **Clippy pedantic** complains about similar names (`state`/`status`),
  missing backticks in doc comments, `assigning_clones`, `float_cmp`,
  `too_many_lines`/`too_many_arguments` (allow with a reason on the
  system).
- **Root-owned files in `target/`** from an earlier sudo run break
  builds: `sudo chown -R` them back.

## Verifying and capturing

```sh
cargo fmt --all --check
cargo clippy --all --workspace --all-targets --features netvisualizer/dev -- -D warnings
cargo test --workspace --features netvisualizer/dev
cargo doc --workspace --no-deps
```

Pictures and interaction runs use the two debugging flags:

- `--no-ui` draws the map alone.
- `--script "STEP; STEP; …"` drives the window: `wait S` (seconds), `move
  X Y`, `click X Y`, `scroll LINES`, `key Escape|Enter|Backspace|c`,
  `text …`, `shot FILE.png`. Pair it with `--exit-after-frames N` (frames
  run about 300 a second in the dev build, so N is in the thousands) and
  start with `wait 4.5` so the window has been floated and sized first.

On Hyprland the window tiles by default; this helper floats it at
1280 × 800 and captures (`hyprctl` 0.56's Lua API; the window class is
`netvisualizer`, from `Window.name`):

```sh
#!/bin/sh
# shot.sh OUT.png [viewer args...]
out="$1"; shift
cd "$(git rev-parse --show-toplevel)" || exit 1
timeout 240 cargo run -q -p netvisualizer --features dev -- "$@" \
  --exit-after-frames "${FRAMES:-420}" --screenshot "$out" > "$out.log" 2>&1 &
pid=$!
sleep 2.5
for try in 1 2 3; do
  hyprctl eval 'local w = hl.get_windows({class="netvisualizer"})[1] if w and not w.floating then hl.dispatch(hl.dsp.window.float(w)) end' >/dev/null 2>&1
  sleep 0.4
  hyprctl eval 'local w = hl.get_windows({class="netvisualizer"})[1] if w then hl.dispatch(hl.dsp.window.resize({x=1280, y=800, window=w})) hl.dispatch(hl.dsp.window.center(w)) end' >/dev/null 2>&1
  sleep 0.4
  size=$(hyprctl clients -j | python3 -c "import json,sys; print(*[c['size'] for c in json.load(sys.stdin) if c['class']=='netvisualizer'][:1])" 2>/dev/null)
  [ "$size" = "[1280, 800]" ] && break
done
wait $pid
grep -iE "panic|error\[" "$out.log" | head -3
```

Example: a demo map, hover a toggle, open the drawer, type a port list:

```sh
FRAMES=4000 ./shot.sh drawer.png --demo --script \
  "wait 4.5; move 300 755; wait 1; shot tip.png; click 640 755; wait 0.5; \
   click 612 640; wait 0.3; text 22,80; key Enter; wait 0.5"
```

Real scans for pictures were run without sudo on the LAN (ping sweep,
names, TCP common ports); say so in the report when you do the same.

## Work remaining (in order)

1. `hud.rs`: the two-line box, the three key-hint buttons, the square
   legend. Delete the round help button.
2. `dock.rs`: the toggle row, the `[ SCAN ]` block, prompt fields, the
   text progress bar, the drawer above the row; `DOCK_INSET` 120.
3. `rail.rs`: `SIDEBAR_WIDTH` 330; the bands and rules; the table columns
   (`Slot.role` text instead of a dot; an `AddrHead` pin updated with the
   width); details at the bottom with raw keys and the footer buttons;
   the `[<]` tab.
4. `help.rs` (title in capitals, caps with radius `BUTTON_RADIUS`),
   `labels.rs` (rounding 0, border colours; fix its tests).
5. `panel.rs`: drop `heading()` if nothing uses it after the switch to
   raw keys; keep `humanize()` only if used.
6. README and `project_spec.md`: the font (JetBrains Mono only), the
   look's name, the module table; keep the spec short.
7. Verify (all four commands), capture idle / scan running / drawer /
   tooltip / collapsed / help at 1280 × 800, and replace the Terminal
   section's mockup on the design page with real captures.
8. Update `graphics-plan-status.md` in the memory directory and write the
   commit message.

## Room to grow

- Milestone 12 (one action per operation): more `[x] NAME` toggles fit
  the row up to about eight at 1280 wide; past that, a second row above
  the first. A per-operation button is a bracketed command (`[ PING ]`)
  beside Scan at a smaller size.
- Milestone 13 (SSH console): a console pane is native to the look; dock
  it as a third box under the legend or as a sheet sliding up over the
  dock, with the same band + rule header.
- Milestone 15 (device types): a glyph or a short code in a fourth table
  column (`TYPE`) and in the details dump; keep the role word as is.

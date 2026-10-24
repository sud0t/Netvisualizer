//! The building blocks the panels are made of: text in the viewer's fonts,
//! a node's painted [`Skin`], buttons and toggle chips with a hover blend,
//! and tooltips.
//!
//! A widget is a lunex layout node with two kinds of children: a skin (the
//! quad that paints the node) and text. Neither child is a layout node of
//! its own: the skin is scaled to the node by shape.rs, and a [`Pin`] keeps
//! the text at the node's left, centre or right. A node that should be as
//! wide as its text carries [`FitText`].

use bevy::ecs::observer::IntoEntityObserver;
use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy::text::{FontWeight, LineBreak, TextLayoutInfo};
use bevy::window::SystemCursorIcon;
use bevy_lunex::UiState;
use bevy_lunex::prelude::*;

use crate::theme;
use crate::ui::shape::{Look, SKIN_Z, ShapeAssets, ShapeMaterial, Skin};

/// Text sits above its node's skin.
pub const TEXT_Z: f32 = 0.5;

/// The viewer's one font, embedded in the binary (see mod.rs): a terminal
/// has a single monospace face.
#[derive(Resource)]
pub struct Fonts {
    pub mono: Handle<Font>,
}

/// Which weight a piece of text is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Mono,
    MonoBold,
}

/// A face at a size, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub face: Face,
    pub px: f32,
}

impl TextStyle {
    /// Body text.
    pub const BODY: TextStyle = TextStyle {
        face: Face::Mono,
        px: theme::BODY_PX,
    };
    /// Panel titles and the toggles: bold, written in capitals by the caller.
    pub const TITLE: TextStyle = TextStyle {
        face: Face::MonoBold,
        px: theme::TITLE_PX,
    };
    pub const LABEL: TextStyle = TextStyle {
        face: Face::MonoBold,
        px: theme::BODY_PX,
    };
    /// Tables, dumps, addresses: a size smaller.
    pub const MONO: TextStyle = TextStyle {
        face: Face::Mono,
        px: theme::MONO_PX,
    };
    pub const SMALL: TextStyle = TextStyle {
        face: Face::Mono,
        px: theme::SMALL_PX,
    };

    /// The width of a character, to size text before it is laid out
    /// (`JetBrains Mono` advances 0.6 em).
    pub fn average_advance(self) -> f32 {
        self.px * 0.6
    }

    /// The width of `text` in this style.
    pub fn width_of(self, text: &str) -> f32 {
        text.chars().count() as f32 * self.average_advance()
    }
}

/// `text` cut to about `max_px` wide in `style`, with an ellipsis when cut.
pub fn truncate(text: &str, max_px: f32, style: TextStyle) -> String {
    let fits = (max_px / style.average_advance()).floor().max(1.0) as usize;
    let count = text.chars().count();
    if count <= fits {
        return text.to_string();
    }
    let keep = fits.saturating_sub(1);
    let mut cut: String = text.chars().take(keep).collect();
    cut.push('…');
    cut
}

/// Where a text child sits in its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Center,
}

/// Keeps a text child at its node's left edge or centre: `pad` pixels in
/// from the edge, `dy` pixels off the vertical centre (down is negative).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Pin {
    pub side: Side,
    pub pad: f32,
    pub dy: f32,
}

impl Pin {
    pub const CENTER: Pin = Pin {
        side: Side::Center,
        pad: 0.0,
        dy: 0.0,
    };

    pub fn left(pad: f32) -> Pin {
        Pin {
            side: Side::Left,
            pad,
            dy: 0.0,
        }
    }

    fn anchor(self) -> Anchor {
        match self.side {
            Side::Left => Anchor::CENTER_LEFT,
            Side::Center => Anchor::CENTER,
        }
    }

    /// The text's position in a node `width` wide.
    fn offset(self, width: f32) -> Vec2 {
        let x = match self.side {
            Side::Left => -width / 2.0 + self.pad,
            Side::Center => 0.0,
        };
        Vec2::new(x, self.dy)
    }
}

/// The node is as wide as its text child plus `pad` on either side (at
/// least `min` wide).
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct FitText {
    pub pad: f32,
    pub min: f32,
}

/// What a node looks like at rest and under the pointer. The hover blend
/// follows lunex's [`UiHover`] state.
#[derive(Component, Debug, Clone)]
pub struct Styled {
    pub skin: Entity,
    pub base: Look,
    pub hover: Look,
    /// The text child and its colours at rest and under the pointer.
    pub text: Option<(Entity, Color, Color)>,
}

/// A toggle that is on or off; the click flips it, [`sync_toggles`] restyles
/// it and rewrites its label (`[x] ARP` or `[ ] ARP`).
#[derive(Component, Debug, Clone)]
pub struct Toggle {
    pub on: bool,
    pub on_look: Look,
    pub off_look: Look,
    pub on_hover: Look,
    pub off_hover: Look,
    pub on_text: Color,
    pub off_text: Color,
    pub on_label: String,
    pub off_label: String,
}

impl Toggle {
    /// The label of a toggle for `name`, on or off.
    pub fn label(name: &str, on: bool) -> String {
        if on {
            format!("[x] {name}")
        } else {
            format!("[ ] {name}")
        }
    }
}

/// Shown near the pointer after it rests on the node a moment.
#[derive(Component, Debug, Clone)]
pub struct Tooltip(pub String);

/// The hover state the widgets use: quick in, slower out.
pub fn hoverable() -> UiHover {
    UiHover::new().forward_speed(16.0).backward_speed(7.0)
}

/// The resources a widget is spawned from.
pub struct Kit<'a> {
    pub fonts: &'a Fonts,
    pub shapes: &'a ShapeAssets,
    pub materials: &'a mut Assets<ShapeMaterial>,
}

impl Kit<'_> {
    pub fn font(&self, style: TextStyle) -> TextFont {
        let weight = match style.face {
            Face::Mono => FontWeight::NORMAL,
            Face::MonoBold => FontWeight::BOLD,
        };
        TextFont::from_font_size(style.px)
            .with_font(self.fonts.mono.clone())
            .with_font_weight(weight)
    }

    /// A line of text, pinned in its node. Never wraps.
    pub fn text(
        &self,
        text: impl Into<String>,
        style: TextStyle,
        color: Color,
        pin: Pin,
    ) -> impl Bundle {
        (
            Text2d::new(text),
            self.font(style),
            TextColor(color),
            TextLayout::new(Justify::Left, LineBreak::NoWrap),
            pin.anchor(),
            pin,
            Transform::from_xyz(0.0, 0.0, TEXT_Z),
        )
    }

    /// The quad that paints a node; `pad` is room for the glow.
    pub fn skin(&mut self, look: Look, pad: f32) -> impl Bundle {
        let material = self.materials.add(ShapeMaterial {
            shape: look.shape(Vec2::ONE, pad),
        });
        (
            Mesh2d(self.shapes.quad.clone()),
            MeshMaterial2d(material),
            Skin { look, pad },
            Transform::from_xyz(0.0, 0.0, SKIN_Z),
        )
    }
}

/// What a button or chip is made of.
pub struct ButtonSpec<'s> {
    pub name: &'static str,
    pub layout: UiLayout,
    pub label: &'s str,
    pub style: TextStyle,
    pub text: Color,
    pub text_hover: Color,
    pub base: Look,
    pub hover: Look,
    /// Room the skin's quad keeps for a glow.
    pub pad: f32,
    /// Size the node to its label, with this much padding on either side.
    pub fit: Option<f32>,
    pub tooltip: Option<&'s str>,
}

impl ButtonSpec<'_> {
    /// A plain button: hairline border, a faint fill under the pointer.
    pub fn plain<'s>(name: &'static str, layout: UiLayout, label: &'s str) -> ButtonSpec<'s> {
        ButtonSpec {
            name,
            layout,
            label,
            style: TextStyle::BODY,
            text: theme::TEXT,
            text_hover: theme::TEXT,
            base: Look::bordered(Color::NONE, theme::HUD_BORDER, theme::BUTTON_RADIUS),
            hover: Look::bordered(
                theme::ROW_HOVER,
                theme::HUD_BORDER_HOVER,
                theme::BUTTON_RADIUS,
            ),
            pad: 0.0,
            fit: None,
            tooltip: None,
        }
    }

    /// A ghost button: text only, a faint fill under the pointer.
    pub fn ghost<'s>(name: &'static str, layout: UiLayout, label: &'s str) -> ButtonSpec<'s> {
        ButtonSpec {
            text: theme::MUTED,
            base: Look::filled(Color::NONE, theme::BUTTON_RADIUS),
            hover: Look::filled(theme::ROW_HOVER, theme::BUTTON_RADIUS),
            ..ButtonSpec::plain(name, layout, label)
        }
    }

    pub fn tooltip<'s>(self, tip: &'s str) -> ButtonSpec<'s>
    where
        Self: 's,
    {
        ButtonSpec {
            tooltip: Some(tip),
            ..self
        }
    }

    pub fn style(mut self, style: TextStyle) -> Self {
        self.style = style;
        self
    }
}

/// Spawn a button under `ui`; `on_click` is an observer of the node, for a
/// `PointerClick` (check `button == PointerButton::Primary` where it matters).
pub fn button<M>(
    ui: &mut ChildSpawnerCommands,
    kit: &mut Kit,
    spec: ButtonSpec,
    on_click: impl IntoEntityObserver<M>,
) -> Entity {
    let mut node = ui.spawn((
        Name::new(spec.name),
        spec.layout,
        hoverable(),
        OnHoverSetCursor::new(SystemCursorIcon::Pointer),
    ));
    if let Some(pad) = spec.fit {
        node.insert(FitText { pad, min: 0.0 });
    }
    if let Some(tip) = spec.tooltip {
        node.insert(Tooltip(tip.to_string()));
    }
    let mut skin = Entity::PLACEHOLDER;
    let mut text = Entity::PLACEHOLDER;
    node.with_children(|n| {
        skin = n.spawn(kit.skin(spec.base, spec.pad)).id();
        text = n
            .spawn(kit.text(spec.label, spec.style, spec.text, Pin::CENTER))
            .id();
    });
    node.insert(Styled {
        skin,
        base: spec.base,
        hover: spec.hover,
        text: Some((text, spec.text, spec.text_hover)),
    });
    node.observe(hover_set::<PointerOver, true>)
        .observe(hover_set::<PointerOut, false>)
        .observe(on_click);
    node.id()
}

/// Spawn a toggle: `[x] NAME` lit while it is on, `[ ] NAME` dim while off.
#[allow(clippy::too_many_arguments)] // a toggle's name, layout, label, state and tip
pub fn toggle<M>(
    ui: &mut ChildSpawnerCommands,
    kit: &mut Kit,
    name: &'static str,
    layout: UiLayout,
    label: &str,
    on: bool,
    tooltip: &str,
    on_click: impl IntoEntityObserver<M>,
) -> Entity {
    let toggle = Toggle {
        on,
        on_look: theme::chip_on(),
        off_look: theme::chip_off(),
        on_hover: theme::chip_on_hover(),
        off_hover: theme::chip_off_hover(),
        on_text: theme::ACCENT_TEXT,
        off_text: theme::MUTED,
        on_label: Toggle::label(label, true),
        off_label: Toggle::label(label, false),
    };
    let (base, hover, text) = toggle.looks();
    let shown = Toggle::label(label, on);
    let spec = ButtonSpec {
        name,
        layout,
        label: &shown,
        style: TextStyle::LABEL,
        text,
        text_hover: text,
        base,
        hover,
        pad: theme::CHIP_GLOW,
        fit: Some(theme::CHIP_PAD_X),
        tooltip: Some(tooltip),
    };
    let entity = button(ui, kit, spec, on_click);
    ui.commands().entity(entity).insert(toggle);
    entity
}

impl Toggle {
    /// (rest, hover, text colour) for the current state.
    fn looks(&self) -> (Look, Look, Color) {
        if self.on {
            (self.on_look, self.on_hover, self.on_text)
        } else {
            (self.off_look, self.off_hover, self.off_text)
        }
    }
}

pub struct WidgetsPlugin;

impl Plugin for WidgetsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            (
                fit_text_nodes.in_set(UiSystems::PreCompute),
                (sync_toggles, blend_styles, pin_texts)
                    .chain()
                    .after(UiSystems::PostCompute),
            ),
        );
    }
}

/// A node with [`FitText`] takes the width of its text (measured by Bevy in
/// physical pixels: divide by the scale factor).
fn fit_text_nodes(
    texts: Query<(&ChildOf, &TextLayoutInfo), Changed<TextLayoutInfo>>,
    mut nodes: Query<(&FitText, &mut UiLayout)>,
) {
    for (child_of, info) in &texts {
        let Ok((fit, mut layout)) = nodes.get_mut(child_of.parent()) else {
            continue;
        };
        if info.size.y == 0.0 {
            continue; // not laid out yet
        }
        let width = (info.size.x / info.scale_factor + 2.0 * fit.pad).max(fit.min);
        set_width(&mut layout, width);
    }
}

/// Set the base width of a window or flow layout.
pub fn set_width(layout: &mut UiLayout, width: f32) {
    if let Some(window) = layout.get_mut_window(UiBase::id()) {
        window.set_width(Ab(width));
    } else if let Some(flow) = layout.get_mut_flow(UiBase::id()) {
        flow.width = UiFlowAxis::from(UiFlowSize::from(Ab(width)));
    }
}

/// A toggle that flipped takes the looks and the label of its new state.
fn sync_toggles(
    mut toggles: Query<(&Toggle, &mut Styled), Changed<Toggle>>,
    mut texts: Query<&mut Text2d>,
) {
    for (toggle, mut styled) in &mut toggles {
        let (base, hover, text) = toggle.looks();
        styled.base = base;
        styled.hover = hover;
        if let Some((entity, rest, under)) = &mut styled.text {
            *rest = text;
            *under = text;
            let label = if toggle.on {
                &toggle.on_label
            } else {
                &toggle.off_label
            };
            if let Ok(mut shown) = texts.get_mut(*entity)
                && shown.0 != *label
            {
                shown.0.clone_from(label);
            }
        }
    }
}

/// Paint each styled node between its rest and hover looks by how far
/// lunex's hover transition has got.
fn blend_styles(
    styled: Query<(&Styled, &UiState), Or<(Changed<UiState>, Changed<Styled>)>>,
    mut skins: Query<&mut Skin>,
    mut colors: Query<&mut TextColor>,
) {
    for (style, state) in &styled {
        let t = state.weight(&UiHover::id()).unwrap_or(0.0);
        if let Ok(mut skin) = skins.get_mut(style.skin) {
            skin.look = style.base.lerp(&style.hover, t);
        }
        if let Some((text, rest, under)) = style.text
            && let Ok(mut color) = colors.get_mut(text)
        {
            let (a, b) = (rest.to_linear(), under.to_linear());
            color.0 = a.mix(&b, t).into();
        }
    }
}

/// Pinned text follows its node's width, and a pin that moved.
fn pin_texts(
    mut texts: Query<(&ChildOf, Ref<Pin>, &mut Transform, &mut Anchor)>,
    nodes: Query<Ref<Dimension>>,
) {
    for (child_of, pin, mut transform, mut anchor) in &mut texts {
        let Ok(dimension) = nodes.get(child_of.parent()) else {
            continue;
        };
        if !(dimension.is_changed() || pin.is_changed()) {
            continue;
        }
        let offset = pin.offset(dimension.x);
        transform.translation.x = offset.x;
        transform.translation.y = offset.y;
        *anchor = pin.anchor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_short_text_and_ellipsises_long_text() {
        assert_eq!(truncate("router.lan", 200.0, TextStyle::BODY), "router.lan");
        // 13 px mono: 7.8 px a character, so 50 px fits 6.
        let cut = truncate("netvisualizer-host.lan", 50.0, TextStyle::BODY);
        assert_eq!(cut, "netvi…");
        assert_eq!(cut.chars().count(), 6);
        assert_eq!(TextStyle::MONO.width_of("1234567890"), 72.0);
        // Monospace 12 px: 7.2 px a character, 72 px fits 10.
        assert_eq!(
            truncate("192.168.100.200", 72.0, TextStyle::MONO),
            "192.168.1…"
        );
        assert_eq!(truncate("ab", 1.0, TextStyle::MONO), "…");
    }

    #[test]
    fn toggle_labels_show_the_state_in_brackets() {
        assert_eq!(Toggle::label("ARP", true), "[x] ARP");
        assert_eq!(Toggle::label("UDP", false), "[ ] UDP");
    }

    #[test]
    fn pins_sit_at_the_sides_of_their_node() {
        assert_eq!(Pin::left(8.0).offset(100.0), Vec2::new(-42.0, 0.0));
        let lowered = Pin {
            dy: -3.0,
            ..Pin::CENTER
        };
        assert_eq!(lowered.offset(100.0), Vec2::new(0.0, -3.0));
        assert_eq!(Pin::left(0.0).anchor(), Anchor::CENTER_LEFT);
        assert_eq!(Pin::CENTER.anchor(), Anchor::CENTER);
    }

    #[test]
    fn set_width_reaches_window_and_flow_layouts() {
        let mut window = UiLayout::window().size((Ab(10.0), Ab(10.0))).pack();
        set_width(&mut window, 42.0);
        let got = window.get_window(UiBase::id()).unwrap().size.get_x();
        assert_eq!(got, UiValue::from(Ab(42.0)));
        let mut flow = UiLayout::flow().width(Ab(10.0)).pack();
        set_width(&mut flow, 42.0);
        assert_eq!(
            flow.get_flow(UiBase::id()).unwrap().width.size,
            UiFlowSize::Fixed(Ab(42.0).into())
        );
    }
}

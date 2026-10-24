//! A single-line text field: click to focus, type, Backspace, Ctrl+V to
//! paste, Ctrl+C to copy, Enter to submit, Escape (or a click elsewhere) to
//! leave. One field has the focus at a time ([`Focus`]); while one does,
//! the fly keys are off (ui.rs reads [`Focus`] into `UiInput::keyboard`).
//!
//! The caret always sits at the end: these fields hold short values (a
//! network, a port list, a filter), which are retyped rather than edited.
//! A field may start with a prompt (`net>`), written dim before the value,
//! the way a shell names what it is asking for.

use arboard::Clipboard;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;
use bevy::text::TextLayoutInfo;
use bevy_lunex::prelude::*;

use crate::ViewerSet;
use crate::controls::command_modifier_held;
use crate::theme;
use crate::ui::shape::{Look, Skin};
use crate::ui::widgets::{Kit, Pin, TextStyle};

/// Text 9 px in from the field's edge.
const FIELD_PAD_X: f32 = 9.0;
/// The caret: a block the width of a character, blinking once a second.
const CARET_W: f32 = 7.0;
const CARET_HEIGHT: f32 = 14.0;
const CARET_PERIOD: f32 = 1.0;

/// The field's value and how it is shown.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct TextField {
    pub value: String,
    /// Written dim before the value (`net>`); empty for none.
    pub prompt: String,
    /// Shown, muted, while the value is empty.
    pub hint: String,
    pub style: TextStyle,
    pub max_len: usize,
    /// Drawn with the error border.
    pub error: bool,
}

impl TextField {
    /// Where the value starts: after the padding and the prompt, if any.
    fn value_x(&self) -> f32 {
        if self.prompt.is_empty() {
            FIELD_PAD_X
        } else {
            FIELD_PAD_X + self.style.width_of(&self.prompt) + self.style.average_advance()
        }
    }
}

/// The field's children.
#[derive(Component, Debug)]
struct FieldParts {
    skin: Entity,
    text: Entity,
    caret: Entity,
}

/// The field with the keyboard, if any.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Focus {
    pub field: Option<Entity>,
}

/// Enter was pressed in this field.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Submitted(pub Entity);

/// Give this field the focus (or take it from every field with `None`).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusField(pub Option<Entity>);

/// Spawn a field under `ui` with its layout node of `layout`; `prompt` is
/// written before the value (empty for none).
#[allow(clippy::too_many_arguments)] // a field's name, layout, prompt, value, hint and style
pub fn text_field(
    ui: &mut ChildSpawnerCommands,
    kit: &mut Kit,
    name: &'static str,
    layout: UiLayout,
    prompt: &str,
    value: &str,
    hint: &str,
    style: TextStyle,
) -> Entity {
    let field = TextField {
        value: value.to_string(),
        prompt: prompt.to_string(),
        hint: hint.to_string(),
        style,
        max_len: 128,
        error: false,
    };
    let mut node = ui.spawn((
        Name::new(name),
        layout,
        OnHoverSetCursor::new(bevy::window::SystemCursorIcon::Text),
    ));
    let mut parts = FieldParts {
        skin: Entity::PLACEHOLDER,
        text: Entity::PLACEHOLDER,
        caret: Entity::PLACEHOLDER,
    };
    node.with_children(|n| {
        parts.skin = n.spawn(kit.skin(field_look(false, false), 0.0)).id();
        if !field.prompt.is_empty() {
            n.spawn(kit.text(
                field.prompt.clone(),
                style,
                theme::MUTED,
                Pin::left(FIELD_PAD_X),
            ));
        }
        let (shown, color) = field.shown();
        parts.text = n
            .spawn(kit.text(shown, style, color, Pin::left(field.value_x())))
            .id();
        parts.caret = n
            .spawn((
                Sprite::from_color(theme::ACCENT, Vec2::new(CARET_W, CARET_HEIGHT)),
                Transform::from_xyz(0.0, 0.0, 0.6),
                Visibility::Hidden,
            ))
            .id();
    });
    node.insert((field, parts));
    node.observe(
        |click: On<PointerClick>, mut focus: MessageWriter<FocusField>| {
            if click.button == PointerButton::Primary {
                focus.write(FocusField(Some(click.event_target())));
            }
        },
    );
    node.id()
}

impl TextField {
    /// What the field shows and in which colour: the value, or the hint.
    fn shown(&self) -> (String, Color) {
        if self.value.is_empty() {
            (self.hint.clone(), theme::MUTED)
        } else {
            (self.value.clone(), theme::TEXT)
        }
    }
}

/// The field's box: the panel's fill, a lit border while focused, the error
/// colour while its value is refused.
fn field_look(focused: bool, error: bool) -> Look {
    let stroke = if error {
        theme::ERROR
    } else if focused {
        theme::ACCENT
    } else {
        theme::HUD_BORDER
    };
    Look::bordered(theme::INPUT_BG, stroke, theme::BUTTON_RADIUS)
}

pub struct TextInputPlugin;

impl Plugin for TextInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Focus>()
            .add_message::<Submitted>()
            .add_message::<FocusField>()
            .add_systems(
                Update,
                (apply_focus, blur_on_click_elsewhere, type_into_focused)
                    .chain()
                    .in_set(ViewerSet::Receive),
            )
            .add_systems(
                PostUpdate,
                (show_values, blink_caret).after(UiSystems::PostCompute),
            );
    }
}

/// The last focus request of the frame wins. A field that is gone loses it.
fn apply_focus(
    mut requests: MessageReader<FocusField>,
    mut focus: ResMut<Focus>,
    fields: Query<(), With<TextField>>,
) {
    if let Some(request) = requests.read().last() {
        let wanted = request.0.filter(|e| fields.get(*e).is_ok());
        if focus.field != wanted {
            focus.field = wanted;
        }
    }
    if focus.field.is_some_and(|e| fields.get(e).is_err()) {
        focus.field = None;
    }
}

/// A click that lands anywhere but on the focused field (or in it) takes
/// the focus away; clicks on the 3D view included.
fn blur_on_click_elsewhere(
    buttons: Res<ButtonInput<MouseButton>>,
    hover: Res<HoverMap>,
    parents: Query<&ChildOf>,
    mut focus: ResMut<Focus>,
) {
    let Some(field) = focus.field else {
        return;
    };
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let on_field = hover.get(&PointerId::Mouse).is_some_and(|hits| {
        hits.keys()
            .any(|e| *e == field || parents.iter_ancestors(*e).any(|a| a == field))
    });
    if !on_field {
        focus.field = None;
    }
}

/// Keys go to the focused field.
fn type_into_focused(
    mut events: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: ResMut<Focus>,
    mut fields: Query<&mut TextField>,
    mut submitted: MessageWriter<Submitted>,
) {
    let Some(entity) = focus.field else {
        events.clear();
        return;
    };
    let Ok(mut field) = fields.get_mut(entity) else {
        return;
    };
    for event in events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        if command_modifier_held(&keys) {
            if let Key::Character(c) = &event.logical_key {
                match c.to_lowercase().as_str() {
                    "v" => {
                        if let Some(pasted) = clipboard_text() {
                            field.insert(&pasted);
                        }
                    }
                    "c" => clipboard_copy(&field.value),
                    _ => {}
                }
            }
            continue;
        }
        match &event.logical_key {
            Key::Backspace => {
                field.value.pop();
            }
            Key::Escape => {
                focus.field = None;
                return;
            }
            Key::Enter => {
                submitted.write(Submitted(entity));
            }
            _ => {
                if let Some(text) = &event.text {
                    field.insert(text);
                }
            }
        }
    }
}

impl TextField {
    /// Append the typed or pasted text, minus control characters and line
    /// breaks, up to the field's length.
    fn insert(&mut self, text: &str) {
        for c in text.chars().filter(|c| !c.is_control()) {
            if self.value.chars().count() >= self.max_len {
                break;
            }
            self.value.push(c);
        }
    }
}

fn clipboard_text() -> Option<String> {
    Clipboard::new().ok()?.get_text().ok()
}

fn clipboard_copy(text: &str) {
    if let Ok(mut clipboard) = Clipboard::new() {
        let _ = clipboard.set_text(text.to_string());
    }
}

/// The text child shows the value (or the hint), the skin shows the focus
/// and the error, and the caret sits after the last character. A handful of
/// fields: checked every frame, written only when something differs.
fn show_values(
    focus: Res<Focus>,
    fields: Query<(Entity, &TextField, &FieldParts, &Dimension)>,
    mut texts: Query<(&mut Text2d, &mut TextColor, &TextLayoutInfo)>,
    mut skins: Query<&mut Skin>,
    mut carets: Query<&mut Transform, With<Sprite>>,
) {
    for (entity, field, parts, dimension) in &fields {
        let focused = focus.field == Some(entity);
        let (shown, color) = field.shown();
        let mut text_width = 0.0;
        if let Ok((mut text, mut text_color, info)) = texts.get_mut(parts.text) {
            if text.0 != shown {
                text.0 = shown;
            }
            if text_color.0 != color {
                text_color.0 = color;
            }
            if !field.value.is_empty() {
                text_width = info.size.x / info.scale_factor;
            }
        }
        let look = field_look(focused, field.error);
        if let Ok(mut skin) = skins.get_mut(parts.skin)
            && skin.look != look
        {
            skin.look = look;
        }
        if focused && let Ok(mut caret) = carets.get_mut(parts.caret) {
            // The block sits right after the last glyph (its transform is
            // its centre).
            let x = -dimension.x / 2.0 + field.value_x() + text_width + 1.0 + CARET_W / 2.0;
            if (caret.translation.x - x).abs() > 0.01 {
                caret.translation.x = x;
            }
        }
    }
}

/// The caret blinks while its field has the focus and is hidden otherwise.
fn blink_caret(
    time: Res<Time>,
    focus: Res<Focus>,
    fields: Query<(Entity, &FieldParts)>,
    mut carets: Query<&mut Visibility>,
) {
    let on = (time.elapsed_secs() % CARET_PERIOD) < CARET_PERIOD / 2.0;
    for (entity, parts) in &fields {
        let Ok(mut visibility) = carets.get_mut(parts.caret) else {
            continue;
        };
        let wanted = if focus.field == Some(entity) && on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

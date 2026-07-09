use schnellui::{
    FocusedInputEvent, RawFocusEvent, RawImeEvent, RawInputState, RawKeyEvent, RawModifiers,
    RawPointerAction, RawPointerButton, RawPointerEvent, RawWheelDelta,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrowserModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
}

impl From<RawModifiers> for BrowserModifiers {
    fn from(value: RawModifiers) -> Self {
        Self {
            shift: value.shift,
            control: value.control,
            alt: value.alt,
            meta: value.super_key,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserKeyEvent {
    /// DOM `KeyboardEvent.key` value.
    pub key: String,
    /// DOM `KeyboardEvent.code` value when SchnellUI can identify it.
    pub code: String,
    pub pressed: bool,
    pub repeat: bool,
    pub modifiers: BrowserModifiers,
    pub text: Option<String>,
}

impl From<RawKeyEvent> for BrowserKeyEvent {
    fn from(value: RawKeyEvent) -> Self {
        Self {
            key: key_name(&value.logical_key),
            code: code_name(value.physical_key),
            pressed: value.state == RawInputState::Pressed,
            repeat: value.repeat,
            modifiers: value.modifiers.into(),
            text: value.text,
        }
    }
}

fn key_name(key: &schnellui::raw_keyboard::Key) -> String {
    use schnellui::raw_keyboard::{Key, NamedKey};
    match key {
        Key::Character(value) => value.to_string(),
        Key::Named(named) => match named {
            NamedKey::Alt => "Alt",
            NamedKey::AltGraph => "AltGraph",
            NamedKey::ArrowDown => "ArrowDown",
            NamedKey::ArrowLeft => "ArrowLeft",
            NamedKey::ArrowRight => "ArrowRight",
            NamedKey::ArrowUp => "ArrowUp",
            NamedKey::Backspace => "Backspace",
            NamedKey::CapsLock => "CapsLock",
            NamedKey::Control => "Control",
            NamedKey::Delete => "Delete",
            NamedKey::End => "End",
            NamedKey::Enter => "Enter",
            NamedKey::Escape => "Escape",
            NamedKey::F1 => "F1",
            NamedKey::F2 => "F2",
            NamedKey::F3 => "F3",
            NamedKey::F4 => "F4",
            NamedKey::F5 => "F5",
            NamedKey::F6 => "F6",
            NamedKey::F7 => "F7",
            NamedKey::F8 => "F8",
            NamedKey::F9 => "F9",
            NamedKey::F10 => "F10",
            NamedKey::F11 => "F11",
            NamedKey::F12 => "F12",
            NamedKey::Home => "Home",
            NamedKey::Insert => "Insert",
            NamedKey::Meta => "Meta",
            NamedKey::PageDown => "PageDown",
            NamedKey::PageUp => "PageUp",
            NamedKey::Shift => "Shift",
            NamedKey::Space => " ",
            NamedKey::Tab => "Tab",
            _ => "Unidentified",
        }
        .to_owned(),
        Key::Dead(_) => "Dead".to_owned(),
        Key::Unidentified(_) => "Unidentified".to_owned(),
    }
}

fn code_name(key: schnellui::raw_keyboard::PhysicalKey) -> String {
    use schnellui::raw_keyboard::PhysicalKey;
    match key {
        PhysicalKey::Code(code) => format!("{code:?}"),
        PhysicalKey::Unidentified(_) => "Unidentified".to_owned(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserMouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

impl From<RawPointerButton> for BrowserMouseButton {
    fn from(value: RawPointerButton) -> Self {
        match value {
            RawPointerButton::Left => Self::Left,
            RawPointerButton::Right => Self::Right,
            RawPointerButton::Middle => Self::Middle,
            RawPointerButton::Back => Self::Back,
            RawPointerButton::Forward => Self::Forward,
            RawPointerButton::Other(value) => Self::Other(value),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BrowserWheelDelta {
    Lines { x: f32, y: f32 },
    Pixels { x: f32, y: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BrowserPointerKind {
    Move,
    Button {
        button: BrowserMouseButton,
        pressed: bool,
    },
    Wheel(BrowserWheelDelta),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrowserPointerEvent {
    pub x: f32,
    pub y: f32,
    pub modifiers: BrowserModifiers,
    pub kind: BrowserPointerKind,
}

impl From<RawPointerEvent> for BrowserPointerEvent {
    fn from(value: RawPointerEvent) -> Self {
        let kind = match value.action {
            RawPointerAction::Move => BrowserPointerKind::Move,
            RawPointerAction::Button { button, state } => BrowserPointerKind::Button {
                button: button.into(),
                pressed: state == RawInputState::Pressed,
            },
            RawPointerAction::Wheel(delta) => BrowserPointerKind::Wheel(match delta {
                RawWheelDelta::Lines { x, y } => BrowserWheelDelta::Lines { x, y },
                RawWheelDelta::Pixels { x, y } => BrowserWheelDelta::Pixels { x, y },
            }),
        };
        Self {
            x: value.position.x,
            y: value.position.y,
            modifiers: value.modifiers.into(),
            kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BrowserInput {
    Key(BrowserKeyEvent),
    Pointer(BrowserPointerEvent),
    Focus(bool),
    Composition { text: String, committed: bool },
}

impl TryFrom<FocusedInputEvent> for BrowserInput {
    type Error = ();

    fn try_from(value: FocusedInputEvent) -> Result<Self, Self::Error> {
        match value {
            FocusedInputEvent::Key(event) => Ok(Self::Key(event.into())),
            FocusedInputEvent::Pointer(event) => Ok(Self::Pointer(event.into())),
            FocusedInputEvent::Focus(event) => Ok(Self::Focus(matches!(
                event,
                RawFocusEvent::WidgetGained | RawFocusEvent::WindowGained
            ))),
            FocusedInputEvent::Ime(RawImeEvent::Preedit { text, .. }) => Ok(Self::Composition {
                text,
                committed: false,
            }),
            FocusedInputEvent::Ime(RawImeEvent::Commit(text)) => Ok(Self::Composition {
                text,
                committed: true,
            }),
            FocusedInputEvent::Ime(RawImeEvent::Enabled | RawImeEvent::Disabled)
            | FocusedInputEvent::Clipboard(_) => Err(()),
        }
    }
}

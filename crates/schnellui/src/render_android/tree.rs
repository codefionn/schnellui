//! The retained node tree a render produces.

use std::collections::BTreeMap;

use schnellui::template::{ChangeHandler, ClickHandler, InputHandler, ToggleHandler};

use super::ops::{prop, Op};

/// A property value.
#[derive(Clone, Debug)]
pub(crate) enum Value {
    Str(String),
    F32(f32),
    I32(i32),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::F32(a), Self::F32(b)) => a.to_bits() == b.to_bits(),
            (Self::I32(a), Self::I32(b)) => a == b,
            _ => false,
        }
    }
}

impl Value {
    pub(crate) fn set_op(&self, id: u32, prop: u8) -> Op {
        match self {
            Self::Str(value) => Op::SetStr {
                id,
                prop,
                value: value.clone(),
            },
            Self::F32(value) => Op::SetF32 {
                id,
                prop,
                value: *value,
            },
            Self::I32(value) => Op::SetI32 {
                id,
                prop,
                value: *value,
            },
        }
    }
}

#[derive(Default)]
pub(crate) struct Handlers {
    pub click: Option<ClickHandler>,
    pub toggle: Option<ToggleHandler>,
    pub change: Option<ChangeHandler>,
    pub input: Option<InputHandler>,
}

/// One node of the rendered tree. This is the [`AndroidTemplate`] node type.
///
/// [`AndroidTemplate`]: super::AndroidTemplate
pub struct AndroidNode {
    /// Zero until the diff assigns one. Ids are never reused within a mount.
    pub(crate) id: u32,
    pub(crate) kind: u8,
    pub(crate) key: Option<String>,
    pub(crate) props: BTreeMap<u8, Value>,
    pub(crate) children: Vec<AndroidNode>,
    pub(crate) handlers: Handlers,
}

impl AndroidNode {
    pub(crate) fn new(kind: u8) -> Self {
        Self {
            id: 0,
            kind,
            key: None,
            props: BTreeMap::new(),
            children: Vec::new(),
            handlers: Handlers::default(),
        }
    }

    pub(crate) fn f32(&mut self, prop: u8, value: f32) {
        self.props.insert(prop, Value::F32(value));
    }

    pub(crate) fn opt_f32(&mut self, prop: u8, value: Option<f32>) {
        if let Some(value) = value {
            self.f32(prop, value);
        }
    }

    pub(crate) fn i32(&mut self, prop: u8, value: i32) {
        self.props.insert(prop, Value::I32(value));
    }

    pub(crate) fn bool(&mut self, prop: u8, value: bool) {
        self.i32(prop, value as i32);
    }

    pub(crate) fn str(&mut self, prop: u8, value: impl Into<String>) {
        self.props.insert(prop, Value::Str(value.into()));
    }

    pub(crate) fn pad(&mut self, left: f32, top: f32, right: f32, bottom: f32) {
        self.f32(prop::PAD_LEFT, left);
        self.f32(prop::PAD_TOP, top);
        self.f32(prop::PAD_RIGHT, right);
        self.f32(prop::PAD_BOTTOM, bottom);
    }

    /// The node with this id, searching the subtree.
    pub(crate) fn find_mut(&mut self, id: u32) -> Option<&mut AndroidNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }
}

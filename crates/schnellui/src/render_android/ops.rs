//! The op buffer: constants, the op type, the encoder and the decoder.
//!
//! The byte layout is fixed by `docs/android-renderer.md`. Integers and floats are
//! little-endian. A string is a `u32` byte length followed by UTF-8.

use std::fmt;

/// First byte of every buffer.
pub const MAGIC: u8 = 0x53;
/// Second byte of every buffer.
pub const VERSION: u8 = 0x01;

/// View kind codes.
pub mod kind {
    pub const ROW: u8 = 1;
    pub const COLUMN: u8 = 2;
    pub const STACK: u8 = 3;
    pub const SCROLL: u8 = 4;
    pub const TEXT: u8 = 5;
    pub const BUTTON: u8 = 6;
    pub const CHECKBOX: u8 = 7;
    pub const SLIDER: u8 = 8;
    pub const TEXT_INPUT: u8 = 9;
    pub const SPACER: u8 = 10;
    pub const SWITCH: u8 = 11;
    pub const RADIO: u8 = 12;
    pub const DROPDOWN: u8 = 13;
    pub const PROGRESS: u8 = 14;
    pub const LOADING: u8 = 15;
    pub const TEXT_AREA: u8 = 16;
    pub const IMAGE: u8 = 17;
    pub const DIVIDER: u8 = 18;
    pub const BADGE: u8 = 19;
    pub const LINK: u8 = 20;
    pub const DIALOG: u8 = 21;
    pub const TAB_BAR: u8 = 22;
    pub const TAB: u8 = 23;
    pub const LIST_ITEM: u8 = 24;
    pub const TABLE: u8 = 25;
    pub const TABLE_ROW: u8 = 26;
    pub const THEME_PROVIDER: u8 = 27;
}

/// Property codes.
pub mod prop {
    pub const WIDTH: u8 = 1;
    pub const HEIGHT: u8 = 2;
    pub const MIN_WIDTH: u8 = 3;
    pub const MIN_HEIGHT: u8 = 4;
    pub const MAX_WIDTH: u8 = 5;
    pub const MAX_HEIGHT: u8 = 6;
    pub const FILL: u8 = 7;
    pub const GROW: u8 = 8;
    pub const BASIS: u8 = 9;
    pub const PAD_LEFT: u8 = 10;
    pub const PAD_TOP: u8 = 11;
    pub const PAD_RIGHT: u8 = 12;
    pub const PAD_BOTTOM: u8 = 13;
    pub const VISIBLE: u8 = 14;
    pub const ENABLED: u8 = 15;
    pub const JUSTIFY: u8 = 20;
    pub const ALIGN: u8 = 21;
    pub const GAP: u8 = 22;
    pub const WRAP: u8 = 23;
    pub const SCROLLBAR: u8 = 24;
    pub const TEXT: u8 = 30;
    pub const TEXT_SIZE: u8 = 31;
    pub const TEXT_WRAP: u8 = 32;
    pub const TEXT_ALIGN: u8 = 33;
    pub const ELLIPSIS: u8 = 34;
    pub const ROLE: u8 = 35;
    pub const A11Y_LABEL: u8 = 36;
    pub const CHECKED: u8 = 40;
    pub const VALUE: u8 = 41;
    pub const MIN: u8 = 42;
    pub const MAX: u8 = 43;
    pub const STEP: u8 = 44;
    pub const PASSWORD: u8 = 45;
    pub const APPEARANCE: u8 = 46;
    /// Dropdown option labels joined with U+001F.
    pub const ITEMS: u8 = 50;
    /// Dropdown selection, -1 for none.
    pub const SELECTED_INDEX: u8 = 52;
    /// Image source: a `data:` URL, a file path or `asset:///name`.
    pub const SRC: u8 = 53;
    /// Link target, opened by the host after CLICK.
    pub const HREF: u8 = 54;
    /// Selected state of a Tab or ListItem.
    pub const SELECTED: u8 = 55;
    /// Size query target: 0 is the parent, any other value the id of an ancestor.
    pub const SHOW_TARGET: u8 = 56;
    pub const SHOW_MIN_W: u8 = 57;
    pub const SHOW_MAX_W: u8 = 58;
    pub const SHOW_MIN_H: u8 = 59;
    pub const SHOW_MAX_H: u8 = 60;
}

/// Event codes, as sent from the host.
pub mod event {
    pub const CLICK: u8 = 1;
    pub const TOGGLE: u8 = 2;
    pub const CHANGE: u8 = 3;
    pub const INPUT: u8 = 4;
    pub const SELECT: u8 = 5;
    pub const DISMISS: u8 = 6;
}

const OP_CREATE: u8 = 0x01;
const OP_SET_STR: u8 = 0x02;
const OP_SET_F32: u8 = 0x03;
const OP_SET_I32: u8 = 0x04;
const OP_CLEAR: u8 = 0x05;
const OP_INSERT: u8 = 0x06;
const OP_REMOVE: u8 = 0x07;
const OP_SET_ROOT: u8 = 0x08;
const OP_CREATE_THEMED: u8 = 0x09;

/// Theme 1 of CREATE_THEMED: light.
pub const THEME_LIGHT: u8 = 1;
/// Theme 2 of CREATE_THEMED: dark.
pub const THEME_DARK: u8 = 2;

/// One decoded op. Kind and prop codes stay raw so a buffer from a newer app
/// still decodes.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Create {
        id: u32,
        kind: u8,
    },
    /// CREATE with a fixed theme, [`THEME_LIGHT`] or [`THEME_DARK`].
    CreateThemed {
        id: u32,
        kind: u8,
        theme: u8,
    },
    SetStr {
        id: u32,
        prop: u8,
        value: String,
    },
    SetF32 {
        id: u32,
        prop: u8,
        value: f32,
    },
    SetI32 {
        id: u32,
        prop: u8,
        value: i32,
    },
    Clear {
        id: u32,
        prop: u8,
    },
    Insert {
        parent: u32,
        child: u32,
        index: u32,
    },
    Remove {
        id: u32,
    },
    SetRoot {
        id: u32,
    },
}

/// Why a buffer did not decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer ended inside the header or an op.
    Truncated,
    BadMagic(u8),
    BadVersion(u8),
    UnknownOp(u8),
    BadUtf8,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("op buffer ended early"),
            Self::BadMagic(b) => write!(f, "bad op buffer magic {b:#04x}"),
            Self::BadVersion(v) => write!(f, "unsupported op buffer version {v}"),
            Self::UnknownOp(c) => write!(f, "unknown op code {c:#04x}"),
            Self::BadUtf8 => f.write_str("op string is not UTF-8"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Encodes ops into a buffer with the two-byte header.
pub fn encode(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + ops.len() * 8);
    out.push(MAGIC);
    out.push(VERSION);
    for op in ops {
        match op {
            Op::Create { id, kind } => {
                out.push(OP_CREATE);
                put_u32(&mut out, *id);
                out.push(*kind);
            }
            Op::CreateThemed { id, kind, theme } => {
                out.push(OP_CREATE_THEMED);
                put_u32(&mut out, *id);
                out.push(*kind);
                out.push(*theme);
            }
            Op::SetStr { id, prop, value } => {
                out.push(OP_SET_STR);
                put_u32(&mut out, *id);
                out.push(*prop);
                put_u32(&mut out, value.len() as u32);
                out.extend_from_slice(value.as_bytes());
            }
            Op::SetF32 { id, prop, value } => {
                out.push(OP_SET_F32);
                put_u32(&mut out, *id);
                out.push(*prop);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Op::SetI32 { id, prop, value } => {
                out.push(OP_SET_I32);
                put_u32(&mut out, *id);
                out.push(*prop);
                out.extend_from_slice(&value.to_le_bytes());
            }
            Op::Clear { id, prop } => {
                out.push(OP_CLEAR);
                put_u32(&mut out, *id);
                out.push(*prop);
            }
            Op::Insert {
                parent,
                child,
                index,
            } => {
                out.push(OP_INSERT);
                put_u32(&mut out, *parent);
                put_u32(&mut out, *child);
                put_u32(&mut out, *index);
            }
            Op::Remove { id } => {
                out.push(OP_REMOVE);
                put_u32(&mut out, *id);
            }
            Op::SetRoot { id } => {
                out.push(OP_SET_ROOT);
                put_u32(&mut out, *id);
            }
        }
    }
    out
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.at.checked_add(n).ok_or(DecodeError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(DecodeError::Truncated)?;
        self.at = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, DecodeError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }
}

/// Decodes a whole buffer. An empty op list is valid.
pub fn decode(bytes: &[u8]) -> Result<Vec<Op>, DecodeError> {
    let mut r = Reader { bytes, at: 0 };
    let magic = r.u8()?;
    if magic != MAGIC {
        return Err(DecodeError::BadMagic(magic));
    }
    let version = r.u8()?;
    if version != VERSION {
        return Err(DecodeError::BadVersion(version));
    }
    let mut ops = Vec::new();
    while r.at < bytes.len() {
        let code = r.u8()?;
        let op = match code {
            OP_CREATE => Op::Create {
                id: r.u32()?,
                kind: r.u8()?,
            },
            OP_CREATE_THEMED => Op::CreateThemed {
                id: r.u32()?,
                kind: r.u8()?,
                theme: r.u8()?,
            },
            OP_SET_STR => Op::SetStr {
                id: r.u32()?,
                prop: r.u8()?,
                value: r.string()?,
            },
            OP_SET_F32 => Op::SetF32 {
                id: r.u32()?,
                prop: r.u8()?,
                value: r.f32()?,
            },
            OP_SET_I32 => Op::SetI32 {
                id: r.u32()?,
                prop: r.u8()?,
                value: r.i32()?,
            },
            OP_CLEAR => Op::Clear {
                id: r.u32()?,
                prop: r.u8()?,
            },
            OP_INSERT => Op::Insert {
                parent: r.u32()?,
                child: r.u32()?,
                index: r.u32()?,
            },
            OP_REMOVE => Op::Remove { id: r.u32()? },
            OP_SET_ROOT => Op::SetRoot { id: r.u32()? },
            other => return Err(DecodeError::UnknownOp(other)),
        };
        ops.push(op);
    }
    Ok(ops)
}

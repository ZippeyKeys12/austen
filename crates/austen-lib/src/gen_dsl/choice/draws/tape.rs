use crate::gen_dsl::choice::draws::Draw;
use crate::gen_dsl::choice::draws::source::{DrawSource, Site};

pub fn site_bytes(width: u16) -> usize {
    usize::from(width.div_ceil(8))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ChoiceTape(pub Vec<u8>);

impl ChoiceTape {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn push(&mut self, value: i128, width: u16) {
        match site_bytes(width) {
            0 => {}
            1 => self.0.push(value as u8),
            2 => self.0.extend_from_slice(&(value as u16).to_le_bytes()),
            4 => self.0.extend_from_slice(&(value as u32).to_le_bytes()),
            8 => self.0.extend_from_slice(&(value as u64).to_le_bytes()),
            want => {
                let bytes = (value as u128).to_le_bytes();
                self.0.extend_from_slice(&bytes[..want.min(16)]);
                for _ in 16..want {
                    self.0.push(0);
                }
            }
        }
    }
}

impl std::ops::Deref for ChoiceTape {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

pub struct DrawTape<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> DrawTape<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    pub fn consumed(&self) -> usize {
        self.cursor
    }
}

impl DrawSource for DrawTape<'_> {
    fn draw(&mut self, site: Site) -> Option<Draw> {
        let want = site_bytes(site.width);
        let end = self.cursor.checked_add(want)?;
        if end > self.bytes.len() {
            return None;
        }
        let taken = &self.bytes[self.cursor..end];
        self.cursor = end;

        let word = match *taken {
            [] => 0,
            [a] => u128::from(a),
            [a, b] => u128::from(u16::from_le_bytes([a, b])),
            [a, b, c, d] => u128::from(u32::from_le_bytes([a, b, c, d])),
            [a, b, c, d, e, f, g, h] => u128::from(u64::from_le_bytes([a, b, c, d, e, f, g, h])),
            _ => {
                let mut word = [0u8; 16];
                let keep = taken.len().min(16);
                word[..keep].copy_from_slice(&taken[..keep]);
                u128::from_le_bytes(word)
            }
        };
        Some(Draw::Bits(word))
    }
}

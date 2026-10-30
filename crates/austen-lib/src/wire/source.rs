use crate::gen_dsl::draws::{Draw, DrawSource, DrawTape, Site, endless, site_bytes};
use serde::{Deserialize, Serialize};

pub const POINTER_READING: &str = "absolute";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteUse {
    pub consumed: u64,
    pub unused: u64,
    pub sampled: u64,
}

struct Accounted<'a> {
    source: &'a mut dyn DrawSource,
    input_bytes: usize,
    consumed: usize,
    sampled: usize,
    handed_over: bool,
}

impl<'a> Accounted<'a> {
    fn new(source: &'a mut dyn DrawSource, input_bytes: usize) -> Self {
        Self {
            source,
            input_bytes,
            consumed: 0,
            sampled: 0,
            handed_over: false,
        }
    }

    fn byte_use(&self) -> ByteUse {
        ByteUse {
            consumed: self.consumed as u64,
            unused: self.input_bytes.saturating_sub(self.consumed) as u64,
            sampled: self.sampled as u64,
        }
    }
}

impl DrawSource for Accounted<'_> {
    fn draw(&mut self, site: Site) -> Option<Draw> {
        let bytes = site_bytes(site.width);
        if !self.handed_over {
            let available = self.input_bytes.saturating_sub(self.consumed);
            if bytes <= available {
                self.consumed += bytes;
            } else {
                self.handed_over = true;
            }
        }
        if self.handed_over {
            self.sampled = self.sampled.saturating_add(bytes);
        }
        self.source.draw(site)
    }
}

pub fn with_source<R>(bytes: &[u8], f: impl FnOnce(&mut dyn DrawSource) -> R) -> R {
    f(&mut endless(DrawTape::new(bytes)))
}

pub fn with_source_accounted<R>(
    bytes: &[u8],
    f: impl FnOnce(&mut dyn DrawSource) -> R,
) -> (R, ByteUse) {
    with_source(bytes, |source| {
        let mut accounted = Accounted::new(source, bytes.len());
        let result = f(&mut accounted);
        (result, accounted.byte_use())
    })
}

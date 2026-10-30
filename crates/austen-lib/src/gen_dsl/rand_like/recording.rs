use std::io::Write;

pub const MAGIC: &str = "AUEN";

pub const VERSION: u8 = 1;

#[must_use]
pub fn header_line() -> String {
    format!("{MAGIC} {VERSION}")
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordingError {
    #[error(
        "not an entropy recording: it does not begin with a `{MAGIC} <version>` \
         line, which is what --record writes. A raw harness input (function and \
         size header plus choice tape) is not entropy and would \
         steer the engine into a different run rather than reproduce one"
    )]
    NotARecording,
    #[error(
        "entropy recording version {found}, but this build writes and reads \
         version {VERSION}: replaying it would reproduce a different run"
    )]
    Version { found: u8 },
    #[error("entropy recording line {line}: not base64 ({reason})")]
    Corrupt { line: usize, reason: &'static str },
}

pub struct Recorder<W: Write> {
    out: W,
}

impl<W: Write> Recorder<W> {
    pub fn new(mut out: W) -> std::io::Result<Self> {
        writeln!(out, "{}", header_line())?;
        Ok(Self { out })
    }

    pub fn push(&mut self, entropy: &[u8]) -> std::io::Result<()> {
        writeln!(self.out, "{}", encode(entropy))
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

pub fn unframe(bytes: &[u8]) -> Result<Vec<Vec<u8>>, RecordingError> {
    let text = std::str::from_utf8(bytes).map_err(|_| RecordingError::NotARecording)?;
    let mut lines = text.lines();
    let header = lines.next().ok_or(RecordingError::NotARecording)?;
    let version = header
        .strip_prefix(MAGIC)
        .map(str::trim)
        .ok_or(RecordingError::NotARecording)?;
    let found: u8 = version.parse().map_err(|_| RecordingError::NotARecording)?;
    if found != VERSION {
        return Err(RecordingError::Version { found });
    }
    lines
        .enumerate()
        .map(|(i, line)| {
            decode(line.trim_end()).map_err(|reason| RecordingError::Corrupt {
                line: i + 2,
                reason,
            })
        })
        .collect()
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const PAD: u8 = b'=';

const NOT_BASE64: u8 = 64;

const REVERSE: [u8; 256] = {
    let mut table = [NOT_BASE64; 256];
    let mut i = 0;
    while i < ALPHABET.len() {
        table[ALPHABET[i] as usize] = i as u8;
        i += 1;
    }
    table
};

#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                char::from(ALPHABET[(n >> (18 - 6 * i)) as usize & 0x3f])
            } else {
                char::from(PAD)
            });
        }
    }
    out
}

fn decode(text: &str) -> Result<Vec<u8>, &'static str> {
    let raw = text.as_bytes();
    if !raw.len().is_multiple_of(4) {
        return Err("length is not a multiple of four");
    }
    let body = raw
        .strip_suffix(&[PAD, PAD])
        .or_else(|| raw.strip_suffix(&[PAD]))
        .unwrap_or(raw);
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut have = 0;
    for &c in body {
        let v = REVERSE[c as usize];
        if v == NOT_BASE64 {
            return Err("character outside the base64 alphabet");
        }
        acc = (acc << 6) | u32::from(v);
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push((acc >> have) as u8);
        }
    }
    if acc & ((1 << have) - 1) != 0 {
        return Err("padding bits are not zero");
    }
    Ok(out)
}

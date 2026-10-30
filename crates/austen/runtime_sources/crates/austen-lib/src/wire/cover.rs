use std::path::Path;

pub const COVER_ENV: &str = "AUSTEN_COVER_OUT";

pub const COVER_MAGIC: u64 = 0x4155_5354_454e_4356;

const HEADER_WORDS: usize = 4;
const HEADER_BYTES: usize = HEADER_WORDS * std::mem::size_of::<u64>();

const SITES_UNKNOWN: u64 = u64::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverPage {
    pub map_bytes: u64,
    pub registered: u64,
    pub sites: Option<u64>,
    pub edges: EdgeSet,
}

impl CoverPage {
    pub fn read(path: &Path) -> Option<CoverPage> {
        Self::from_bytes(&std::fs::read(path).ok()?)
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<CoverPage> {
        if bytes.len() < HEADER_BYTES {
            return None;
        }
        let word = |i: usize| {
            let at = i * 8;
            u64::from_ne_bytes(bytes[at..at + 8].try_into().expect("8 bytes"))
        };
        if word(0) != COVER_MAGIC {
            return None;
        }
        let map_bytes = word(1);
        let want = usize::try_from(map_bytes).ok()?.div_ceil(8);
        let bitset = bytes.get(HEADER_BYTES..)?;
        if bitset.len() != want {
            return None;
        }
        let sites = word(3);
        Some(CoverPage {
            map_bytes,
            registered: word(2),
            sites: (sites != SITES_UNKNOWN).then_some(sites),
            edges: EdgeSet::from_bits(bitset),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_BYTES + self.edges.bits.len());
        for word in [
            COVER_MAGIC,
            self.map_bytes,
            self.registered,
            self.sites.unwrap_or(SITES_UNKNOWN),
        ] {
            out.extend_from_slice(&word.to_ne_bytes());
        }
        out.extend_from_slice(&self.edges.bits);
        out
    }

    pub fn collision_free(&self) -> Option<bool> {
        self.sites
            .map(|s| s == self.registered && s <= self.map_bytes)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeSet {
    bits: Vec<u8>,
}

impl EdgeSet {
    pub fn with_capacity_bits(bits: usize) -> EdgeSet {
        EdgeSet {
            bits: vec![0; bits.div_ceil(8)],
        }
    }

    pub fn from_map(map: &[u8]) -> EdgeSet {
        let mut set = EdgeSet::with_capacity_bits(map.len());
        for (i, byte) in map.iter().enumerate() {
            if *byte != 0 {
                set.bits[i / 8] |= 1 << (i % 8);
            }
        }
        set
    }

    fn from_bits(bits: &[u8]) -> EdgeSet {
        EdgeSet {
            bits: bits.to_vec(),
        }
    }

    pub fn len(&self) -> usize {
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|b| *b == 0)
    }

    pub fn contains(&self, i: u32) -> bool {
        let (byte, bit) = (i as usize / 8, i % 8);
        self.bits.get(byte).is_some_and(|b| b & (1 << bit) != 0)
    }

    pub fn insert(&mut self, i: u32) {
        let byte = i as usize / 8;
        if byte >= self.bits.len() {
            self.bits.resize(byte + 1, 0);
        }
        self.bits[byte] |= 1 << (i % 8);
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        self.bits.iter().enumerate().flat_map(|(byte, bits)| {
            (0..8u32)
                .filter_map(move |bit| (bits & (1 << bit) != 0).then_some((byte as u32) * 8 + bit))
        })
    }

    pub fn absorb(&mut self, other: &EdgeSet) -> bool {
        if other.bits.len() > self.bits.len() {
            self.bits.resize(other.bits.len(), 0);
        }
        let mut grew = false;
        for (mine, theirs) in self.bits.iter_mut().zip(&other.bits) {
            grew |= *theirs & !*mine != 0;
            *mine |= *theirs;
        }
        grew
    }

    pub fn absorb_map(&mut self, map: &[u8]) -> bool {
        let needed = map.len().div_ceil(8);
        if needed > self.bits.len() {
            self.bits.resize(needed, 0);
        }
        let mut grew = false;
        let (chunks, tail) = map.as_chunks::<8>();
        let (groups, rest) = chunks.as_chunks::<8>();
        for (group_at, group) in groups.iter().enumerate() {
            let word = group
                .iter()
                .fold(0u64, |word, chunk| word | u64::from_ne_bytes(*chunk));
            if word == 0 {
                continue;
            }
            for (within, chunk) in group.iter().enumerate() {
                grew |= absorb_word(&mut self.bits[group_at * 8 + within], chunk);
            }
        }
        for (within, chunk) in rest.iter().enumerate() {
            grew |= absorb_word(&mut self.bits[groups.len() * 8 + within], chunk);
        }
        if !tail.is_empty() {
            let byte = map.len() / 8;
            let mut mask = 0u8;
            for (bit, slot) in tail.iter().enumerate() {
                if *slot != 0 {
                    mask |= 1 << bit;
                }
            }
            let mine = &mut self.bits[byte];
            grew |= mask & !*mine != 0;
            *mine |= mask;
        }
        grew
    }
}

#[inline]
fn absorb_word(mine: &mut u8, chunk: &[u8; 8]) -> bool {
    if u64::from_ne_bytes(*chunk) == 0 {
        return false;
    }
    let mut mask = 0u8;
    for (bit, slot) in chunk.iter().enumerate() {
        if *slot != 0 {
            mask |= 1 << bit;
        }
    }
    let grew = mask & !*mine != 0;
    *mine |= mask;
    grew
}

impl FromIterator<u32> for EdgeSet {
    fn from_iter<I: IntoIterator<Item = u32>>(iter: I) -> EdgeSet {
        let mut set = EdgeSet::default();
        for i in iter {
            set.insert(i);
        }
        set
    }
}

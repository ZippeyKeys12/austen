use std::hash::Hasher;

use ::fnv::FnvHasher;

pub fn fnv64(bytes: &[u8]) -> u64 {
    let mut hasher = FnvHasher::default();
    hasher.write(bytes);
    hasher.finish()
}

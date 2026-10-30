use super::*;
use austen_lib::spec::eval::integer;

const ACCESS_CHUNK_BYTES: u64 = 64 * 1024;
const FOREIGN_PROBE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) struct Range {
    first: u64,
    stride: u64,
    count: u128,
}

impl Range {
    pub(super) fn new(
        base: u64,
        stride: u64,
        key: &BaseType,
        domain: &std::ops::Range<i128>,
    ) -> Option<Self> {
        if domain.is_empty() || !matches!(key, BaseType::Integer | BaseType::Bits(_, _)) {
            return None;
        }
        for endpoint in [domain.start, domain.end - 1] {
            if integer(index_value(key, endpoint).ok()?)? != endpoint {
                return None;
            }
        }
        Some(Self {
            first: base.wrapping_add(stride.wrapping_mul(domain.start as u64)),
            stride,
            count: domain.end.abs_diff(domain.start),
        })
    }
}

impl Reader<'_> {
    pub(super) fn range_permitted(
        &self,
        permission: &IndexTerm,
        q: &(Sym, BaseType),
        domain: &std::ops::Range<i128>,
        env: &mut SymMap<Value>,
    ) -> bool {
        let (Ok(first), Ok(last)) = (
            index_value(&q.1, domain.start),
            index_value(&q.1, domain.end - 1),
        ) else {
            return false;
        };
        let previous = env.insert(q.0.clone(), first);
        let first = self.eval(permission, env).and_then(boolean);
        env.insert(q.0.clone(), last);
        let last = self.eval(permission, env).and_then(boolean);
        restore(env, &q.0, previous);
        first == Some(true) && last == Some(true)
    }

    pub(super) fn take_quantified(
        &mut self,
        ct: &Sctype,
        init: Init,
        expected: &BaseType,
        origin: PointerOrigin,
        range: Range,
    ) -> Result<bool, WalkError> {
        debug_assert!(!self.phase.is_pre());
        check_owned_request(ct, expected)
            .map_err(|defect| WalkError::Unsupported(Unsupported::new(defect.to_string())))?;
        if range.stride == 0 || size_of(self.module, ct) != Some(range.stride) {
            return Ok(false);
        }
        let step = self.step;
        if range.first == 0
            || (range.stride == 1 && range.count > (1u128 << 64) - u128::from(range.first))
        {
            return Err(WalkError::Failure(CommonFailure::NullTake { step }));
        }
        let Some(size) = range
            .count
            .checked_mul(u128::from(range.stride))
            .and_then(|size| u64::try_from(size).ok())
        else {
            return Ok(false);
        };
        if range.first.checked_add(size).is_none() {
            return Ok(false);
        }
        let extent = OwnershipExtent {
            base: range.first,
            size,
        };

        let count = size / range.stride;
        let mut cuts = vec![0, count];
        let mut cut_at = |address: u128| {
            if address >= u128::from(extent.base) {
                let index = (address - u128::from(extent.base)) / u128::from(range.stride);
                if index < u128::from(count) {
                    let index = index as u64;
                    cuts.extend([index, index + 1]);
                }
            }
        };
        for (_, object) in self.tracked() {
            cut_at(u128::from(object.base));
            cut_at(u128::from(object.base) + u128::from(object.size));
        }
        for claim in &self.taken {
            cut_at(u128::from(claim.base));
            cut_at(u128::from(claim.base) + u128::from(claim.size));
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut pieces = Vec::with_capacity(cuts.len() - 1);
        for pair in cuts.windows(2) {
            let base = extent.base + pair[0] * range.stride;
            let located = self.check_extent(
                OwnershipExtent {
                    base,
                    size: range.stride,
                },
                origin,
            )?;
            pieces.push((
                OwnershipExtent {
                    base,
                    size: (pair[1] - pair[0]) * range.stride,
                },
                located,
            ));
        }
        if init != Init::Uninit {
            return Ok(false);
        }

        let mut foreign_bytes = 0;
        for (piece, located) in pieces {
            if located == Located::Held {
                continue;
            }
            let direct = matches!(located, Located::Raw { .. });
            let mut offset = 0;
            while offset < piece.size {
                let size = ACCESS_CHUNK_BYTES.min(piece.size - offset);
                let addr = piece.base + offset;
                if direct {
                    self.memory.read_into(addr, size, &mut self.scratch)?;
                    self.memory.write_back(addr, &self.scratch)?;
                } else {
                    if size > FOREIGN_PROBE_BYTES - foreign_bytes {
                        return self
                            .unsupported("quantified resource exceeds the foreign probe budget");
                    }
                    foreign_bytes += size;
                    self.memory.read_foreign(addr, size, &mut self.scratch)?;
                    self.memory.write_back_foreign(addr, &self.scratch)?;
                }
                offset += size;
            }
        }
        self.record_extent(extent);
        Ok(true)
    }
}

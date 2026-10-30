pub fn backtrack_total<'a>(pairs: impl IntoIterator<Item = (&'a str, u64)>) -> u64 {
    pairs
        .into_iter()
        .filter(|(reason, _)| *reason != super::RETRIES_EXHAUSTED)
        .map(|(_, count)| count)
        .sum()
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rejections {
    buckets: Vec<(&'static str, u64)>,
}

impl From<crate::gen_dsl::RejectTally> for Rejections {
    fn from(tally: crate::gen_dsl::RejectTally) -> Self {
        Self::from_pairs(tally.nonzero())
    }
}

impl Rejections {
    pub fn add(&mut self, pairs: impl IntoIterator<Item = (&'static str, u64)>) {
        for (reason, count) in pairs {
            if count == 0 {
                continue;
            }
            if let Some((_, existing)) = self.buckets.iter_mut().find(|(key, _)| *key == reason) {
                *existing = existing.saturating_add(count);
            } else {
                self.buckets.push((reason, count));
            }
        }
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (&'static str, u64)>) -> Self {
        Self {
            buckets: pairs.into_iter().filter(|(_, n)| *n > 0).collect(),
        }
    }

    pub fn nonzero(&self) -> &[(&'static str, u64)] {
        &self.buckets
    }

    pub fn total(&self) -> u64 {
        self.buckets.iter().map(|(_, n)| n).sum()
    }

    pub fn backtracks(&self) -> u64 {
        backtrack_total(self.buckets.iter().copied())
    }
}

use std::sync::{Arc, Mutex};

use super::{Rng, SplitMix64};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "recorded input has only {available} of the 8 bytes required for leading decision {decision}"
)]
pub struct TruncatedDecision {
    pub decision: usize,
    pub available: usize,
}

#[derive(Debug)]
pub struct Entropy {
    tape: Vec<u8>,
    at: usize,
    replayed_input: bool,
    queued: std::collections::VecDeque<Vec<u8>>,
    prng: SplitMix64,
    buf: u64,
    have: u8,
    log: Option<Arc<Mutex<Vec<u8>>>>,
    history: Vec<u64>,
    replay_at: Option<usize>,
}

pub struct InputDecisions<'a> {
    entropy: &'a mut Entropy,
    replayed: bool,
    next: usize,
}

impl InputDecisions<'_> {
    pub fn resolve(
        &mut self,
        choose: impl FnOnce(&mut dyn Rng) -> u64,
    ) -> Result<u64, TruncatedDecision> {
        let decision = if self.replayed {
            let available = self.entropy.tape.len().saturating_sub(self.entropy.at);
            let Some(bytes) = self
                .entropy
                .tape
                .get(self.entropy.at..self.entropy.at.saturating_add(8))
            else {
                return Err(TruncatedDecision {
                    decision: self.next,
                    available,
                });
            };
            self.entropy.at += 8;
            u64::from_le_bytes(bytes.try_into().expect("an eight-byte slice"))
        } else {
            let mut unrecorded = Unrecorded(self.entropy);
            choose(&mut unrecorded)
        };
        if let Some(log) = &self.entropy.log {
            locked(log).extend_from_slice(&decision.to_le_bytes());
        }
        self.next += 1;
        Ok(decision)
    }
}

#[derive(Debug, Clone)]
pub struct RecordingLog(Arc<Mutex<Vec<u8>>>);

fn locked(log: &Mutex<Vec<u8>>) -> std::sync::MutexGuard<'_, Vec<u8>> {
    log.lock().expect("a log holder cannot panic mid-write")
}

impl RecordingLog {
    #[must_use]
    pub fn line(&self) -> Vec<u8> {
        locked(&self.0).clone()
    }
}

impl Entropy {
    pub fn seeded(seed: u64) -> Self {
        Self::replay_inputs(Vec::new(), seed)
    }

    pub fn replay(tape: Vec<u8>, seed: u64) -> Self {
        Self::replay_inputs(vec![tape], seed)
    }

    pub fn replay_inputs(inputs: Vec<Vec<u8>>, seed: u64) -> Self {
        Self {
            tape: Vec::new(),
            at: 0,
            replayed_input: false,
            queued: inputs.into(),
            prng: SplitMix64::new(seed),
            buf: 0,
            have: 0,
            log: None,
            history: Vec::new(),
            replay_at: None,
        }
    }

    #[must_use]
    pub fn recording(mut self) -> (Self, RecordingLog) {
        let log = Arc::clone(
            self.log
                .get_or_insert_with(|| Arc::new(Mutex::new(Vec::new()))),
        );
        (self, RecordingLog(log))
    }

    pub fn start_input(&mut self) {
        self.begin_input();
    }

    pub fn start_input_decisions(&mut self) -> InputDecisions<'_> {
        let replayed = self.begin_input();
        InputDecisions {
            entropy: self,
            replayed,
            next: 0,
        }
    }

    pub fn start_input_with_decision(
        &mut self,
        choose: impl FnOnce(&mut dyn Rng) -> u64,
    ) -> Result<u64, TruncatedDecision> {
        self.start_input_decisions().resolve(choose)
    }

    fn begin_input(&mut self) -> bool {
        let replayed = match self.queued.pop_front() {
            Some(next) => {
                self.tape = next;
                true
            }
            None => {
                self.tape.clear();
                false
            }
        };
        self.replayed_input = replayed;
        self.at = 0;
        if let Some(log) = &self.log {
            locked(log).clear();
        }
        self.history.clear();
        self.replay_at = None;
        replayed
    }

    pub fn inputs_left(&self) -> usize {
        self.queued.len()
    }

    fn byte(&mut self) -> u8 {
        if self.at < self.tape.len() {
            let b = self.tape[self.at];
            self.at += 1;
            return b;
        }
        if self.have == 0 {
            self.buf = self.prng.next_u64();
            self.have = 8;
        }
        let b = (self.buf & 0xff) as u8;
        self.buf >>= 8;
        self.have -= 1;
        b
    }

    fn unrecorded_u64(&mut self) -> u64 {
        if let Some(eight) = self.tape.get(self.at..self.at + 8) {
            self.at += 8;
            u64::from_le_bytes(eight.try_into().expect("an eight-byte slice"))
        } else if self.at >= self.tape.len() && self.have == 0 {
            self.prng.next_u64()
        } else {
            let mut bytes = [0u8; 8];
            for b in &mut bytes {
                *b = self.byte();
            }
            u64::from_le_bytes(bytes)
        }
    }
    #[inline]
    fn word(&mut self, fresh: impl FnOnce(&mut Self) -> u64) -> u64 {
        if let Some(i) = self.replay_at {
            if let Some(word) = self.history.get(i).copied() {
                self.replay_at = Some(i + 1);
                return word;
            }
            self.replay_at = None;
        }
        let word = fresh(self);
        self.history.push(word);
        if let Some(log) = &self.log {
            locked(log).extend_from_slice(&word.to_le_bytes());
        }
        word
    }
}

struct Unrecorded<'a>(&'a mut Entropy);

impl Rng for Unrecorded<'_> {
    fn next_u64(&mut self) -> u64 {
        self.0.unrecorded_u64()
    }
}

impl Rng for Entropy {
    fn next_u64(&mut self) -> u64 {
        self.word(Self::unrecorded_u64)
    }

    fn decide(&mut self, choose: &mut dyn FnMut() -> bool) -> bool {
        self.word(|entropy| {
            if entropy.replayed_input {
                entropy.unrecorded_u64()
            } else {
                u64::from(choose())
            }
        }) & 1
            != 0
    }
}

impl super::Rewind for Entropy {
    type Mark = EntropyMark;

    fn save(&self) -> EntropyMark {
        EntropyMark(self.replay_at.unwrap_or(self.history.len()))
    }

    fn restore(&mut self, mark: EntropyMark) {
        debug_assert!(
            mark.0 <= self.history.len(),
            "restoring to a mark this input never reached"
        );
        self.replay_at = Some(mark.0);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntropyMark(usize);

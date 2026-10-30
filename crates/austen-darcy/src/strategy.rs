use std::time::Duration;

use crate::smt::MaxSatEngine;

#[derive(Default)]
pub(crate) struct Strategy {
    costs: [Option<f64>; 2],
    observations: u64,
    preferred: usize,
    invalid_repairs: u8,
    redraws_since_probe: u8,
}

impl Strategy {
    pub(crate) fn choose_redraw(&self) -> bool {
        self.invalid_repairs >= 4 && self.redraws_since_probe < 31
    }

    pub(crate) fn observe_redraw(&mut self, redraw: bool) {
        self.redraws_since_probe = if redraw {
            self.redraws_since_probe.saturating_add(1)
        } else {
            0
        };
    }

    pub(crate) fn observe_repair_model(&mut self, valid: bool) {
        if valid {
            self.invalid_repairs = 0;
            self.redraws_since_probe = 0;
        } else {
            self.invalid_repairs = self.invalid_repairs.saturating_add(1).min(4);
        }
    }

    pub(crate) fn choose_wmax(&mut self) -> bool {
        if self.observations < 4 {
            return self.observations % 2 == 1;
        }
        if let [Some(maxres), Some(wmax)] = self.costs {
            let costs = [maxres, wmax];
            let alternate = 1 - self.preferred;
            if costs[alternate] < costs[self.preferred] * 0.85 {
                self.preferred = alternate;
            }
        }
        let probe = self.observations.is_multiple_of(32);
        (self.preferred == 1) ^ probe
    }

    pub(crate) fn observe(
        &mut self,
        engine: MaxSatEngine,
        elapsed: Duration,
        useful: bool,
        failure_cost: Duration,
    ) {
        let elapsed = if useful {
            elapsed
        } else {
            elapsed.max(failure_cost)
        };
        let seconds = elapsed.as_secs_f64().max(1e-9);
        let index = usize::from(engine == MaxSatEngine::WMax);
        self.costs[index] = Some(match self.costs[index] {
            Some(previous) => previous * 0.75 + seconds * 0.25,
            None => seconds,
        });
        self.observations = self.observations.saturating_add(1);
    }
}

//! Bounded overlapping keyword windows. Continuous speaker playback need not
//! become silent to submit a check. At most one HTTP check is in flight.
#[derive(Default)]
pub struct WakeWindow {
    samples: Vec<i16>,
    since_check: usize,
    sample_rate: u32,
}
impl WakeWindow {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn push(&mut self, samples: &[i16], sr: u32, available: bool) -> Option<Vec<i16>> {
        if sr == 0 {
            return None;
        }
        if self.sample_rate != sr {
            self.clear();
            self.sample_rate = sr;
        }
        self.samples.extend_from_slice(samples);
        let cap = sr as usize * 3;
        if self.samples.len() > cap {
            self.samples.drain(..self.samples.len() - cap);
        }
        self.since_check = self.since_check.saturating_add(samples.len());
        let hop = sr as usize * 4 / 5;
        if !available || self.samples.len() < hop || self.since_check < hop {
            return None;
        }
        self.since_check = 0;
        // Energy is an optimization, never sufficient evidence for interruption.
        let energy = self
            .samples
            .iter()
            .map(|&s| (s as f64 / 32768.0).powi(2))
            .sum::<f64>()
            / self.samples.len() as f64;
        if energy.sqrt() < 0.008 {
            return None;
        }
        Some(self.samples.clone())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn continuous_playback_is_checked_without_waiting_for_silence() {
        let mut w = WakeWindow::default();
        assert!(w.push(&vec![1000; 12799], 16000, true).is_none());
        assert_eq!(w.push(&[1000], 16000, true).unwrap().len(), 12800);
        assert_eq!(
            w.push(&vec![1000; 12800], 16000, true).unwrap().len(),
            25600
        );
    }
    #[test]
    fn pending_request_bounds_memory_and_does_not_launch_more() {
        let mut w = WakeWindow::default();
        for _ in 0..100 {
            assert!(w.push(&vec![1000; 1600], 16000, false).is_none());
        }
        assert_eq!(w.samples.len(), 48000);
        assert_eq!(w.push(&[1000], 16000, true).unwrap().len(), 48000);
    }
    #[test]
    fn silence_and_old_session_samples_are_not_submitted() {
        let mut w = WakeWindow::default();
        assert!(w.push(&vec![0; 48000], 16000, true).is_none());
        w.push(&vec![1000; 16000], 16000, false);
        w.clear();
        assert!(w.push(&[1000], 16000, true).is_none());
        assert!(w.push(&[1000], 48000, true).is_none());
    }
}

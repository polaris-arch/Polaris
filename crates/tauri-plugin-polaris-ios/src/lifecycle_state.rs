//! Receipt ordering is independent of Tauri, NE and process/PID assumptions.

#[derive(Default, Debug)]
pub(crate) struct LifecycleState {
    generation: u64,
    pending: bool,
    running: Option<bool>,
}

impl LifecycleState {
    pub(crate) fn begin(&mut self) -> Result<u64, &'static str> {
        if self.pending {
            return Err("An iOS VPN operation is already in progress");
        }
        self.generation += 1;
        self.pending = true;
        self.running = None;
        Ok(self.generation)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn finish(&mut self, generation: u64, running: Option<bool>) -> bool {
        if self.generation != generation || !self.pending {
            return false;
        }
        self.pending = false;
        self.running = running;
        true
    }

    pub(crate) fn observe(&mut self, generation: u64, running: Option<bool>) -> bool {
        if self.generation != generation || self.pending {
            return false;
        }
        self.running = running;
        true
    }

    pub(crate) fn running(&self) -> bool {
        self.running == Some(true)
    }
}

#[cfg(test)]
mod tests;

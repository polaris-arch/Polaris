//! Receipt ordering is independent of Tauri, NE and process/PID assumptions.

#[derive(Default, Debug)]
pub(crate) struct LifecycleState {
    generation: u64,
    pending: bool,
    running: Option<bool>,
    pending_start: Option<(u64, String)>,
    revoked_shared_generation: u64,
}

impl LifecycleState {
    pub(crate) fn begin(&mut self) -> Result<u64, &'static str> {
        if self.pending || self.pending_start.is_some() {
            return Err("An iOS VPN operation is already in progress");
        }
        self.generation += 1;
        self.pending = true;
        self.running = None;
        self.pending_start = None;
        Ok(self.generation)
    }

    pub(crate) fn begin_start(
        &mut self,
        shared_generation: u64,
        request_id: String,
    ) -> Result<u64, &'static str> {
        if shared_generation == 0 || shared_generation <= self.revoked_shared_generation {
            return Err("StartCancelled: the iOS start generation was revoked before dispatch");
        }
        let generation = self.begin()?;
        self.pending_start = Some((shared_generation, request_id));
        Ok(generation)
    }

    /// Capture the unique pending start at or before this Stop's atomic claim.
    /// A shared successor may still be waiting for the TS gate and have no native
    /// request yet. A delayed Stop(A)
    /// cannot revoke B, including when B reached native before this hook did.
    pub(crate) fn revoke_start_through(
        &mut self,
        captured_previous_generation: u64,
    ) -> Option<String> {
        self.revoked_shared_generation = self
            .revoked_shared_generation
            .max(captured_previous_generation);
        if !self
            .pending_start
            .as_ref()
            .is_some_and(|(generation, _)| *generation <= captured_previous_generation)
        {
            return None;
        }
        let (_, request_id) = self.pending_start.take()?;
        self.generation += 1;
        self.pending = false;
        self.running = None;
        Some(request_id)
    }

    pub(crate) fn accepts_receipt(&self, generation: u64, shared_generation: u64) -> bool {
        self.generation == generation
            && !self.pending
            && self.pending_start.is_none()
            && shared_generation > self.revoked_shared_generation
    }

    /// Losing the Rust receiver cannot undo native work. Keep its exact request
    /// addressable by user Stop until an actual native terminal callback arrives.
    pub(crate) fn abandon(&mut self, generation: u64) -> bool {
        if self.generation != generation || !self.pending {
            return false;
        }
        self.pending = false;
        self.running = None;
        true
    }

    pub(crate) fn native_terminal(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.pending_start = None;
        true
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn finish(&mut self, generation: u64, running: Option<bool>) -> bool {
        if self.generation != generation || !self.pending {
            return false;
        }
        self.pending = false;
        self.pending_start = None;
        self.running = running;
        true
    }

    pub(crate) fn observe(&mut self, generation: u64, running: Option<bool>) -> bool {
        if self.generation != generation || self.pending || self.pending_start.is_some() {
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

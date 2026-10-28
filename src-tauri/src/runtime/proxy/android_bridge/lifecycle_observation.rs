//! Availability observations, never native retirement/custody evidence.
use super::AndroidExactTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum TerminalState {
    Unavailable,
    Closed,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TerminalEvent {
    pub run_id: String,
    pub birth_nonce: String,
    pub state: TerminalState,
}

#[derive(Default)]
pub(super) struct Observation {
    current: Option<Run>,
    pending: Option<Run>,
}

struct Run {
    id: String,
    birth: Option<String>,
    terminal: Option<TerminalState>,
}

impl Observation {
    pub fn begin(&mut self, id: &str) {
        self.pending = Some(Run {
            id: id.into(),
            birth: None,
            terminal: None,
        });
    }

    pub fn abort(&mut self, id: &str) {
        if self.pending.as_ref().is_some_and(|r| r.id == id) {
            self.pending = None;
        }
    }

    pub fn confirm(&mut self, target: AndroidExactTarget) -> bool {
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        if !target.is_valid()
            || pending.id != target.run_id
            || pending
                .birth
                .as_ref()
                .is_some_and(|b| b != &target.birth_nonce)
        {
            return false;
        }
        let mut run = self.pending.take().expect("checked pending run");
        run.birth = Some(target.birth_nonce);
        self.current = Some(run); // A terminal arriving before the start ACK stays terminal.
        true
    }

    pub fn observe(&mut self, event: TerminalEvent) {
        let target = AndroidExactTarget {
            run_id: event.run_id,
            birth_nonce: event.birth_nonce,
        };
        if !target.is_valid() {
            return;
        }
        for run in [&mut self.current, &mut self.pending].into_iter().flatten() {
            if run.id != target.run_id
                || run.birth.as_ref().is_some_and(|b| b != &target.birth_nonce)
            {
                continue;
            }
            run.birth = Some(target.birth_nonce.clone());
            if run.terminal != Some(TerminalState::Closed) {
                run.terminal = Some(event.state);
            }
        }
    }

    pub fn target(&self) -> Option<AndroidExactTarget> {
        let run = self.current.as_ref()?;
        Some(AndroidExactTarget {
            run_id: run.id.clone(),
            birth_nonce: run.birth.clone()?,
        })
    }

    pub fn clear_exact(&mut self, target: Option<&AndroidExactTarget>) {
        if target.is_some() && self.target().as_ref() == target {
            self.current = None;
        }
    }

    pub fn running(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|r| r.birth.is_some() && r.terminal.is_none())
    }
}

#[cfg(test)]
mod tests;

//! Legacy diagnostic interface. A cached pathname is not an execution handle.
//! Never launch a pathname supplied by the updater or PID observation merely to
//! obtain a version string: hash-then-reopen would still leave a replacement race.
//! Until an owned executable-object probe exists, an empty line means unknown.
use std::path::Path;

#[derive(Default)]
pub(crate) struct CoreVersionProbe {
    _private: (),
}

impl CoreVersionProbe {
    /// Diagnostic unknown, never bundled-version or runtime identity evidence.
    pub(crate) fn first_line(&self, _bin: &Path) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests;

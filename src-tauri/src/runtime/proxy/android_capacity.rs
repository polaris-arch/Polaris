//! Explicit capacity rejection for one invocation; no process-wide error override.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapacityClosed;

impl CapacityClosed {
    pub(crate) fn from_code(code: Option<&str>) -> Option<Self> {
        (code == Some(super::code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED)).then_some(Self)
    }

    #[cfg(any(target_os = "android", test))]
    pub(crate) fn spawn_error(self) -> polaris_core_supervisor::SpawnError {
        polaris_core_supervisor::SpawnError::Spawn {
            bin: std::path::PathBuf::from("android-libbox"),
            source: std::io::Error::other(self),
        }
    }

    pub(crate) fn from_spawn(error: &polaris_core_supervisor::SpawnError) -> Option<Self> {
        match error {
            polaris_core_supervisor::SpawnError::Spawn { source, .. } => {
                source.get_ref()?.downcast_ref::<Self>().copied()
            }
        }
    }
}

impl std::fmt::Display for CapacityClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("本次运行的生命周期记录已满，请完全关闭并重新启动应用后重试。")
    }
}

impl std::error::Error for CapacityClosed {}

/// Existing desktop checker diagnostics keep their original value and behavior.
#[derive(Debug)]
pub enum CheckFailure {
    Rejected(String),
    #[allow(
        dead_code,
        reason = "constructed by Android checkers and host fake admission tests"
    )]
    CapacityClosed(CapacityClosed),
}

impl std::fmt::Display for CheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(message) => f.write_str(message),
            Self::CapacityClosed(error) => error.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::runtime::proxy::android_capacity::CapacityClosed;

    #[test]
    fn only_exact_typed_code_is_capacity() {
        assert_eq!(
            CapacityClosed::from_code(Some(
                super::super::code::ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED
            )),
            Some(CapacityClosed)
        );
        for code in [
            None,
            Some("STARTUP_FAILED"),
            Some("ANDROID_NATIVE_ADMISSION_CLOSED"),
            Some("ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED "),
            Some("完全关闭并重新启动应用"),
        ] {
            assert_eq!(CapacityClosed::from_code(code), None);
        }
    }

    #[test]
    fn typed_spawn_source_survives_but_same_raw_message_does_not_classify() {
        assert_eq!(
            CapacityClosed::from_spawn(&CapacityClosed.spawn_error()),
            Some(CapacityClosed)
        );
        let text = CapacityClosed.to_string();
        let raw = polaris_core_supervisor::SpawnError::Spawn {
            bin: "android-libbox".into(),
            source: std::io::Error::other(text),
        };
        assert_eq!(CapacityClosed::from_spawn(&raw), None);
    }
}

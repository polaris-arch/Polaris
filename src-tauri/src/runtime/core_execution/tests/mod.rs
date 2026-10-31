use super::*;
// This test-only producer deliberately has no AdmittedPayload or Prepared.
// It exercises lifecycle/cancellation fixtures, never trusted-execution claims.
#[cfg(test)]
pub(crate) struct LifecycleFixtureSpawner;
#[cfg(test)]
pub(crate) use LifecycleFixtureSpawner as TokioSpawner;
#[cfg(test)]
pub(crate) struct FixtureSpawned {
    pub child: crate::runtime::core_child::Child,
}
#[cfg(test)]
impl LifecycleFixtureSpawner {
    pub fn new() -> Self {
        Self
    }
    pub fn spawn(
        &self,
        request: SpawnRequest,
    ) -> Result<FixtureSpawned, polaris_core_supervisor::SpawnError> {
        use std::process::Stdio;
        let mut command = tokio::process::Command::new(&request.binary);
        command.args(request.argv()).stdin(Stdio::null());
        if let Some(dir) = request.working_dir {
            command.current_dir(dir);
        }
        match &request.stdio {
            StdioPolicy::Discard => {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
            StdioPolicy::Drain(_) => {
                command.stdout(Stdio::piped()).stderr(Stdio::piped());
            }
        }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child =
            command
                .spawn()
                .map_err(|source| polaris_core_supervisor::SpawnError::Spawn {
                    bin: request.binary,
                    source,
                })?;
        if let StdioPolicy::Drain(sink) = request.stdio {
            sink(
                Box::new(child.stdout.take().expect("fixture stdout")),
                Box::new(child.stderr.take().expect("fixture stderr")),
            );
        }
        Ok(FixtureSpawned {
            child: child.into(),
        })
    }
}

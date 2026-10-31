#[cfg(test)]
#[derive(Debug)]
pub(crate) enum Child {
    Admitted(polaris_core_supervisor::CustodiedChild),
    Fixture(tokio::process::Child),
}
#[cfg(test)]
impl From<polaris_core_supervisor::CustodiedChild> for Child {
    fn from(c: polaris_core_supervisor::CustodiedChild) -> Self {
        Self::Admitted(c)
    }
}
#[cfg(test)]
impl From<tokio::process::Child> for Child {
    fn from(c: tokio::process::Child) -> Self {
        Self::Fixture(c)
    }
}
#[cfg(test)]
impl Child {
    pub fn id(&self) -> Option<u32> {
        match self {
            Self::Admitted(c) => c.id(),
            Self::Fixture(c) => c.id(),
        }
    }
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self {
            Self::Admitted(c) => c.try_wait(),
            Self::Fixture(c) => c.try_wait(),
        }
    }
    pub fn start_kill(&mut self) -> std::io::Result<()> {
        match self {
            Self::Admitted(c) => c.start_kill(),
            Self::Fixture(c) => c.start_kill(),
        }
    }
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        match self {
            Self::Admitted(c) => c.wait().await,
            Self::Fixture(c) => c.wait().await,
        }
    }
    pub async fn kill(&mut self) -> std::io::Result<()> {
        match self {
            Self::Admitted(c) => c.kill().await,
            Self::Fixture(c) => c.kill().await,
        }
    }
    #[cfg(windows)]
    pub fn raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        match self {
            Self::Admitted(c) => c.raw_handle(),
            Self::Fixture(c) => c.raw_handle(),
        }
    }
}

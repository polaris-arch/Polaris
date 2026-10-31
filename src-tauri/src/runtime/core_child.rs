//! Factory-owned production Child; raw stand-ins exist only in App unit tests.
#[cfg(not(test))]
pub(crate) use polaris_core_supervisor::CustodiedChild as Child;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::Child;

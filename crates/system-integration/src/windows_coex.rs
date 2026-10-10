//! Windows read-only source facade. No helper protocol or network writer.
//! API queries are synchronous; callers must retain their blocking-worker admission.
//! Query completeness/compartment are unproved, never defaulted to 1 or all-host.
use crate::coexistence::windows::*;
use polaris_config_engine::builder::coexistence::{AddressFamily, Fact};
#[cfg(windows)]
mod native;

pub struct NativeWindowsSource;
impl WindowsFactSource for NativeWindowsSource {
    fn adapters_and_addresses(
        &self,
    ) -> (
        Fact<ReadRows<WindowsAdapterObservation>>,
        Fact<ReadRows<WindowsAddressObservation>>,
    ) {
        #[cfg(windows)]
        {
            native::adapters_and_addresses()
        }
        #[cfg(not(windows))]
        {
            (
                Fact::Unknown("native Windows source unavailable".into()),
                Fact::Unknown("native Windows source unavailable".into()),
            )
        }
    }
    fn routes(&self, family: AddressFamily) -> Fact<ReadRows<WindowsRouteObservation>> {
        #[cfg(windows)]
        {
            native::routes(family)
        }
        #[cfg(not(windows))]
        {
            let _ = family;
            Fact::Unknown("native Windows source unavailable".into())
        }
    }
    fn ras(&self) -> Fact<ReadRows<WindowsRasObservation>> {
        #[cfg(windows)]
        {
            native::ras()
        }
        #[cfg(not(windows))]
        {
            Fact::Unknown("native Windows source unavailable".into())
        }
    }
}

/// Scope metadata is unavailable in the supported native query contracts.
#[cfg(any(windows, test))]
pub(crate) fn observed<T>(rows: Vec<T>) -> Fact<ReadRows<T>> {
    Fact::Known(ReadRows {
        rows,
        complete: Fact::Unknown("native read does not prove complete compartment coverage".into()),
        compartment: Fact::Unknown("native query compartment not established".into()),
        error: Fact::Known(None),
    })
}

/// Own-buffer check before dereferencing native linked-list pointers/strings.
#[cfg(any(windows, test))]
pub(crate) fn bounded_region(
    base: usize,
    capacity: usize,
    pointer: usize,
    size: usize,
    alignment: usize,
) -> bool {
    alignment != 0
        && pointer.is_multiple_of(alignment)
        && pointer >= base
        && pointer
            .checked_add(size)
            .zip(base.checked_add(capacity))
            .is_some_and(|(end, limit)| end <= limit)
}

#[cfg(test)]
mod tests;

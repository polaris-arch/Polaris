use super::*;
use crate::coexistence::windows::{collect_windows, private_ras_name};
use std::sync::Mutex;
struct Fixture {
    calls: Mutex<Vec<&'static str>>,
}
impl WindowsFactSource for Fixture {
    fn adapters_and_addresses(
        &self,
    ) -> (
        Fact<ReadRows<WindowsAdapterObservation>>,
        Fact<ReadRows<WindowsAddressObservation>>,
    ) {
        self.calls.lock().unwrap().push("GAA");
        (observed(vec![]), observed(vec![]))
    }
    fn routes(&self, family: AddressFamily) -> Fact<ReadRows<WindowsRouteObservation>> {
        self.calls
            .lock()
            .unwrap()
            .push(if family == AddressFamily::V4 {
                "v4"
            } else {
                "v6"
            });
        if family == AddressFamily::V4 {
            observed(vec![])
        } else {
            Fact::Unknown("injected permission denied".into())
        }
    }
    fn ras(&self) -> Fact<ReadRows<WindowsRasObservation>> {
        self.calls.lock().unwrap().push("RAS");
        observed(vec![WindowsRasObservation {
            name: Fact::Known(".12025550123".into()),
            all_users: true,
            interface: Fact::Unknown("unassociated".into()),
        }])
    }
}
#[test]
fn fixed_four_reads_preserve_five_sources_failed_family_and_redacted_ras_row() {
    let source = Fixture {
        calls: Mutex::default(),
    };
    let input = collect_windows(&source);
    assert_eq!(
        *source.calls.lock().unwrap(),
        vec!["GAA", "v4", "v6", "RAS"]
    );
    assert!(matches!(input.routes6, Fact::Unknown(_)));
    let Fact::Known(ras) = &input.ras else {
        panic!("RAS row erased")
    };
    assert_eq!(ras.rows.len(), 1);
    assert!(matches!(ras.rows[0].name, Fact::Unknown(_)));
    assert!(matches!(ras.complete, Fact::Unknown(_)));
    assert!(matches!(ras.compartment, Fact::Unknown(_)));
    assert!(!format!("{input:?}").contains("12025550123"));
    let Fact::Known(v4) = input.routes4 else {
        panic!("independent source lost")
    };
    assert!(matches!(v4.proven_empty(), Fact::Unknown(_)));
}
#[test]
fn dot_phone_names_never_enter_diagnostic_facts() {
    for value in [".12025550123", " .+44 7700 900123", ".unknown"] {
        assert!(matches!(private_ras_name(value), Fact::Unknown(_)));
    }
    assert_eq!(private_ras_name("VPN"), Fact::Known("VPN".into()));
}
#[test]
fn own_buffer_bounds_alignment_and_overflow_fail_closed() {
    assert!(bounded_region(16, 32, 24, 8, 8));
    for (base, capacity, pointer, size, align) in [
        (16, 32, 15, 8, 1),
        (16, 32, 24, 32, 8),
        (16, 32, 25, 8, 8),
        (16, 32, 24, 8, 0),
        (usize::MAX - 4, 16, usize::MAX - 2, 8, 1),
    ] {
        assert!(!bounded_region(base, capacity, pointer, size, align));
    }
}

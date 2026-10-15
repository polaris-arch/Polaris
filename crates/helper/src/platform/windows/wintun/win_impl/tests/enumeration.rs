use super::*;
use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
use windows_sys::Win32::NetworkManagement::IpHelper::GAA_FLAG_SKIP_FRIENDLY_NAME;

#[test]
fn ffi_flags_keep_friendly_names_and_real_filter_observes_them() {
    for name in ["polaris-tun0", "Polaris-TUN1", "Ethernet"] {
        let mut wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut calls = 0;
        let found = list_matching_adapters_with(|flags, addresses, size| {
            calls += 1;
            assert_eq!(
                flags & GAA_FLAG_SKIP_FRIENDLY_NAME,
                0,
                "both sizing and enumeration must request the field used for matching"
            );
            if addresses.is_null() {
                *size = std::mem::size_of::<IP_ADAPTER_ADDRESSES_LH>() as u32;
                return ERROR_BUFFER_OVERFLOW;
            }
            assert!(*size as usize >= std::mem::size_of::<IP_ADAPTER_ADDRESSES_LH>());
            // SAFETY: production allocated the requested aligned buffer.
            // The UTF-16 name remains alive through traversal; Next is null.
            unsafe {
                addresses.write(std::mem::zeroed());
                (*addresses).FriendlyName = wide.as_mut_ptr();
            }
            NO_ERROR
        })
        .unwrap();
        assert_eq!(calls, 2);
        let expected = if adapter_name_is_probeable(name) {
            vec![name.to_owned()]
        } else {
            vec![]
        };
        assert_eq!(found, expected);
    }
}

#[test]
fn ffi_enumeration_errors_remain_errors_instead_of_absent() {
    let error = list_matching_adapters_with(|_, _, _| ERROR_ACCESS_DENIED).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
    let error = list_matching_adapters_with(|_, addresses, size| {
        if addresses.is_null() {
            *size = std::mem::size_of::<IP_ADAPTER_ADDRESSES_LH>() as u32;
            ERROR_BUFFER_OVERFLOW
        } else {
            ERROR_ACCESS_DENIED
        }
    })
    .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
}

#[test]
#[ignore = "requires an existing adapter named by POLARIS_TEST_ADAPTER_NAME; read-only live host proxy adapter inventory"]
fn live_existing_polaris_adapter_is_visible() {
    let name = std::env::var("POLARIS_TEST_ADAPTER_NAME")
        .expect("set the name of an already-existing polaris-* adapter");
    assert!(adapter_name_is_probeable(&name));
    let adapters = WinAdapterProbe.list_matching_adapters().unwrap();
    assert!(
        adapters
            .iter()
            .any(|found| found.eq_ignore_ascii_case(&name)),
        "existing adapter {name} was not returned: {adapters:?}"
    );
}

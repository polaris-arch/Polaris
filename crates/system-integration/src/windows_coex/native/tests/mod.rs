use super::*;
#[test]
fn windows_target_abi_records_match_documented_x64_layout_without_os_calls() {
    assert_eq!(size_of::<SOCKADDR_IN>(), 16);
    assert_eq!(size_of::<SOCKADDR_IN6>(), 28);
    assert_eq!(size_of::<SOCKADDR_INET>(), 28);
    assert_eq!(size_of::<RASCONNW>(), 1388);
    assert_eq!(align_of::<RASCONNW>(), 4);
    assert_eq!(std::mem::offset_of!(RASCONNW, szEntryName), 12);
    assert_eq!(std::mem::offset_of!(RASCONNW, dwFlags), 1360);
    assert_eq!(std::mem::offset_of!(MIB_IPFORWARD_TABLE2, Table), 8);
}
#[test]
#[allow(
    unsafe_code,
    reason = "constructs C ABI fixture records only inside owned test allocation; no OS calls"
)]
fn bounded_native_adapter_fixture_preserves_both_indices_ipv6_scope_and_partial_address_failure() {
    let mut buffer = Buffer::new(4096).unwrap();
    // SAFETY: disjoint aligned fixture offsets fit fully inside this live initialized allocation.
    unsafe {
        let base = buffer.cells.as_mut_ptr().cast::<u8>();
        let p4 = base.add(512).cast::<IP_ADAPTER_UNICAST_ADDRESS_LH>();
        let p6 = base.add(640).cast::<IP_ADAPTER_UNICAST_ADDRESS_LH>();
        let s4 = base.add(768).cast::<SOCKADDR_IN>();
        let s6 = base.add(800).cast::<SOCKADDR_IN6>();
        let mut ipv4 = SOCKADDR_IN {
            sin_family: AF_INET,
            ..Default::default()
        };
        ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes([10, 77, 2, 9]);
        s4.write(ipv4);
        let mut ipv6 = SOCKADDR_IN6 {
            sin6_family: AF_INET6,
            ..Default::default()
        };
        ipv6.sin6_addr.u.Byte = [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x12, 0x34];
        ipv6.Anonymous.sin6_scope_id = 42;
        s6.write(ipv6);
        let mut a4 = IP_ADAPTER_UNICAST_ADDRESS_LH::default();
        a4.Anonymous.Anonymous.Length = size_of::<IP_ADAPTER_UNICAST_ADDRESS_LH>() as u32;
        a4.Address.lpSockaddr = s4.cast();
        a4.Address.iSockaddrLength = 16;
        a4.OnLinkPrefixLength = 24;
        a4.Next = p6;
        p4.write(a4);
        let mut a6 = a4;
        a6.Next = std::ptr::null_mut();
        a6.Address.lpSockaddr = s6.cast();
        a6.Address.iSockaddrLength = 28;
        a6.OnLinkPrefixLength = 64;
        p6.write(a6);
        let mut adapter = IP_ADAPTER_ADDRESSES_LH::default();
        adapter.Anonymous1.Anonymous.Length = size_of::<IP_ADAPTER_ADDRESSES_LH>() as u32;
        adapter.Anonymous1.Anonymous.IfIndex = 17;
        adapter.Ipv6IfIndex = 42;
        adapter.Luid.Value = u64::MAX;
        adapter.FirstUnicastAddress = p4;
        base.cast::<IP_ADAPTER_ADDRESSES_LH>().write(adapter);
        let (roster, addresses) = decode_adapters(&buffer).unwrap();
        let addresses = addresses.unwrap();
        assert!(matches!(roster[0].interface.if_index, Fact::Unknown(_)));
        assert_eq!(addresses[0].interface.if_index, Fact::Known(17));
        assert_eq!(addresses[1].interface.if_index, Fact::Known(42));
        assert_eq!(addresses[1].scope_id, Fact::Known(42));
        assert_eq!(addresses[1].address.to_string(), "fe80::1234");
        a6.Next = p6;
        p6.write(a6);
        let (roster, addresses) = decode_adapters(&buffer).unwrap();
        assert_eq!(roster.len(), 1);
        assert!(addresses.unwrap_err().contains("cycle"));
        a6.Next = std::ptr::null_mut();
        a6.Anonymous.Anonymous.Length = u32::MAX;
        p6.write(a6);
        let (roster, addresses) = decode_adapters(&buffer).unwrap();
        assert_eq!(roster.len(), 1);
        assert!(addresses.unwrap_err().contains("length"));
    }
}
#[test]
fn packed_ras_fixture_masks_phone_and_ignores_logon_luid() {
    let mut record = RASCONNW {
        dwSize: size_of::<RASCONNW>() as u32,
        ..Default::default()
    };
    let mut name = [0u16; 257];
    for (slot, unit) in name.iter_mut().zip(".12025550123".encode_utf16()) {
        *slot = unit;
    }
    record.szEntryName = name;
    record.dwFlags = RASCF_AllUsers;
    record.luid.LowPart = 123;
    let row = ras_row(record).unwrap();
    assert!(matches!(row.name, Fact::Unknown(_)));
    assert!(matches!(row.interface, Fact::Unknown(_)));
    assert!(row.all_users);
    assert!(!format!("{row:?}").contains("12025550123"));
}

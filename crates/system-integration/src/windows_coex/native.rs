//! Fixed IP Helper/RAS reads in the App process. No service or network writer.
use super::{bounded_region, observed};
use crate::coexistence::windows::*;
use crate::coexistence::{
    MAX_INTERFACE_ADDRESSES, MAX_INTERFACE_ROWS, MAX_ROUTE_ROWS, MAX_SOURCE_BYTES,
};
use polaris_config_engine::builder::coexistence::{AddressFamily, Fact};
use std::collections::BTreeSet;
use std::mem::{align_of, size_of};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::Rras::{RASCF_AllUsers, RasEnumConnectionsW, RASCONNW};
use windows::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6, SOCKADDR_INET,
};

fn missing<T>(reason: &str) -> Fact<T> {
    Fact::Unknown(reason.into())
}
fn api_error(api: &str, code: u32) -> String {
    format!("Windows {api} failed (code {code})")
}
struct Buffer {
    cells: Vec<u64>,
    bytes: usize,
}
impl Buffer {
    fn new(bytes: usize) -> Result<Self, String> {
        if bytes == 0 || bytes > MAX_SOURCE_BYTES {
            return Err("native buffer size limit exceeded".into());
        }
        Ok(Self {
            cells: vec![0; bytes.div_ceil(8)],
            bytes,
        })
    }
    fn base(&self) -> usize {
        self.cells.as_ptr() as usize
    }
    fn contains<T>(&self, pointer: *const T) -> bool {
        bounded_region(
            self.base(),
            self.bytes,
            pointer as usize,
            size_of::<T>(),
            align_of::<T>(),
        )
    }
    #[allow(
        unsafe_code,
        reason = "copies only a size/alignment checked own-buffer POD record"
    )]
    fn read<T: Copy>(&self, pointer: *const T) -> Result<T, String> {
        if !self.contains(pointer) {
            return Err("native record outside bounded buffer".into());
        }
        // SAFETY: range and alignment checked against this live initialized allocation.
        Ok(unsafe { pointer.read() })
    }
    #[allow(
        unsafe_code,
        reason = "bounded UTF16 reads from the live own GAA buffer"
    )]
    fn wide(&self, pointer: *const u16) -> Fact<String> {
        if pointer.is_null() {
            return missing("native string missing");
        }
        let mut units = Vec::new();
        for i in 0..2048usize {
            let Some(address) = (pointer as usize).checked_add(i * 2) else {
                return missing("native string outside buffer");
            };
            let p = address as *const u16;
            if !self.contains(p) {
                return missing("native string outside buffer");
            }
            // SAFETY: checked in-allocation aligned u16, allocation is live.
            let unit = unsafe { p.read() };
            if unit == 0 {
                return String::from_utf16(&units)
                    .map(Fact::Known)
                    .unwrap_or_else(|_| missing("invalid native UTF16"));
            }
            units.push(unit);
        }
        missing("native string length limit exceeded")
    }
}

#[allow(
    unsafe_code,
    reason = "fixed readonly GAA with bounded aligned owned allocation and bounded retries"
)]
fn adapter_buffer() -> Result<Buffer, String> {
    let flags = GAA_FLAG_INCLUDE_ALL_INTERFACES
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER;
    let mut wanted = 15000usize;
    for _ in 0..3 {
        let mut buffer = Buffer::new(wanted)?;
        let mut bytes = u32::try_from(wanted).map_err(|_| "native buffer size overflow")?;
        // SAFETY: fixed family/flags; all output records are written into live aligned owned capacity.
        let code = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC.0),
                flags,
                None,
                Some(buffer.cells.as_mut_ptr().cast()),
                &mut bytes,
            )
        };
        if code == 0 {
            if bytes as usize > buffer.bytes {
                return Err("GAA returned length beyond buffer".into());
            }
            return Ok(buffer);
        }
        if code != 111 {
            return Err(api_error("GetAdaptersAddresses", code));
        }
        wanted = bytes as usize;
    }
    Err("GAA buffer growth retry limit exceeded".into())
}

#[allow(
    unsafe_code,
    reason = "family-tagged POD sockaddr union copies; no host query"
)]
fn socket_bits(value: SOCKADDR_INET) -> Result<(AddressFamily, IpAddr, Fact<u32>), String> {
    // SAFETY: native family discriminates the union; each selected POD member was supplied by the API.
    unsafe {
        if value.si_family == AF_INET {
            let v = value.Ipv4;
            Ok((
                AddressFamily::V4,
                IpAddr::V4(Ipv4Addr::from(v.sin_addr.S_un.S_addr.to_ne_bytes())),
                missing("IPv4 has no IPv6 scope id"),
            ))
        } else if value.si_family == AF_INET6 {
            let v = value.Ipv6;
            Ok((
                AddressFamily::V6,
                IpAddr::V6(Ipv6Addr::from(v.sin6_addr.u.Byte)),
                Fact::Known(v.Anonymous.sin6_scope_id),
            ))
        } else {
            Err("unsupported native sockaddr family".into())
        }
    }
}
fn address_bits(
    buffer: &Buffer,
    address: windows::Win32::Networking::WinSock::SOCKET_ADDRESS,
) -> Result<(AddressFamily, IpAddr, Fact<u32>), String> {
    let family = buffer.read(address.lpSockaddr.cast::<u16>())?;
    let (value, required) = if family == AF_INET.0 {
        (
            SOCKADDR_INET {
                Ipv4: buffer.read(address.lpSockaddr.cast::<SOCKADDR_IN>())?,
            },
            size_of::<SOCKADDR_IN>(),
        )
    } else if family == AF_INET6.0 {
        (
            SOCKADDR_INET {
                Ipv6: buffer.read(address.lpSockaddr.cast::<SOCKADDR_IN6>())?,
            },
            size_of::<SOCKADDR_IN6>(),
        )
    } else {
        return Err("unsupported native address family".into());
    };
    if usize::try_from(address.iSockaddrLength)
        .ok()
        .is_none_or(|len| len < required)
    {
        return Err("native sockaddr length invalid".into());
    }
    socket_bits(value)
}
fn addresses_for(
    buffer: &Buffer,
    entry: IP_ADAPTER_ADDRESSES_LH,
    reference: &WindowsInterfaceRef,
    v4_index: u32,
) -> Result<Vec<WindowsAddressObservation>, String> {
    let mut pointer = entry.FirstUnicastAddress;
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    while !pointer.is_null() {
        if rows.len() == MAX_INTERFACE_ADDRESSES || !seen.insert(pointer as usize) {
            return Err("address limit or native list cycle".into());
        }
        let row = buffer.read(pointer)?;
        let length = unicast_length(row);
        if length < size_of::<IP_ADAPTER_UNICAST_ADDRESS_LH>()
            || !bounded_region(
                buffer.base(),
                buffer.bytes,
                pointer as usize,
                length,
                align_of::<IP_ADAPTER_UNICAST_ADDRESS_LH>(),
            )
        {
            return Err("native unicast record length invalid".into());
        }
        let (family, address, scope_id) = address_bits(buffer, row.Address)?;
        let mut interface = reference.clone();
        interface.if_index = Fact::Known(if family == AddressFamily::V4 {
            v4_index
        } else {
            entry.Ipv6IfIndex
        });
        rows.push(WindowsAddressObservation {
            interface,
            family,
            address,
            prefix_len: Fact::Known(row.OnLinkPrefixLength),
            scope_id,
        });
        pointer = row.Next;
    }
    Ok(rows)
}
#[allow(
    unsafe_code,
    reason = "reads documented length/index POD union after whole record bounds check"
)]
fn adapter_header(entry: IP_ADAPTER_ADDRESSES_LH) -> (usize, u32, u64) {
    // SAFETY: both union overlays are documented POD views of this copied valid native record.
    unsafe {
        (
            entry.Anonymous1.Anonymous.Length as usize,
            entry.Anonymous1.Anonymous.IfIndex,
            entry.Luid.Value,
        )
    }
}
#[allow(
    unsafe_code,
    reason = "reads documented POD unicast length after whole record bounds check"
)]
fn unicast_length(row: IP_ADAPTER_UNICAST_ADDRESS_LH) -> usize {
    // SAFETY: documented length overlay of copied record.
    unsafe { row.Anonymous.Anonymous.Length as usize }
}
type AdapterDecode = (
    Vec<WindowsAdapterObservation>,
    Result<Vec<WindowsAddressObservation>, String>,
);
fn decode_adapters(buffer: &Buffer) -> Result<AdapterDecode, String> {
    let mut pointer = buffer.cells.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    let mut seen = BTreeSet::new();
    let mut adapters = Vec::new();
    let mut addresses = Vec::new();
    let mut address_error = None;
    while !pointer.is_null() {
        if adapters.len() == MAX_INTERFACE_ROWS || !seen.insert(pointer as usize) {
            return Err("adapter limit or native list cycle".into());
        }
        let entry = buffer.read(pointer)?;
        let (length, v4_index, luid) = adapter_header(entry);
        if length < size_of::<IP_ADAPTER_ADDRESSES_LH>()
            || !bounded_region(
                buffer.base(),
                buffer.bytes,
                pointer as usize,
                length,
                align_of::<IP_ADAPTER_ADDRESSES_LH>(),
            )
        {
            return Err("native adapter record length unsupported".into());
        }
        let interface = WindowsInterfaceRef {
            alias: buffer.wide(entry.FriendlyName.0),
            luid: Fact::Known(luid),
            if_index: missing("adapter index has no single address family"),
        };
        adapters.push(WindowsAdapterObservation {
            interface: interface.clone(),
            if_type: Fact::Known(entry.IfType),
            description: buffer.wide(entry.Description.0),
        });
        match addresses_for(buffer, entry, &interface, v4_index) {
            Ok(rows) => addresses.extend(rows),
            Err(e) => address_error = Some(e),
        }
        pointer = entry.Next;
    }
    Ok((adapters, address_error.map_or(Ok(addresses), Err)))
}
pub(super) fn adapters_and_addresses() -> (
    Fact<ReadRows<WindowsAdapterObservation>>,
    Fact<ReadRows<WindowsAddressObservation>>,
) {
    match adapter_buffer().and_then(|b| decode_adapters(&b)) {
        Ok((adapters, addresses)) => (
            observed(adapters),
            addresses.map(observed).unwrap_or_else(Fact::Unknown),
        ),
        Err(e) => (Fact::Unknown(e.clone()), Fact::Unknown(e)),
    }
}

struct MibOwner(*mut MIB_IPFORWARD_TABLE2);
impl Drop for MibOwner {
    #[allow(
        unsafe_code,
        reason = "releases exactly the successful original IP Helper allocation"
    )]
    fn drop(&mut self) {
        // SAFETY: this guard exclusively owns the successful table until drop; no later borrow survives.
        unsafe {
            FreeMibTable(self.0.cast());
        }
    }
}
#[allow(
    unsafe_code,
    reason = "fixed readonly IP Helper query; count checked before slice; RAII owns table"
)]
pub(super) fn routes(family: AddressFamily) -> Fact<ReadRows<WindowsRouteObservation>> {
    let operation = || -> Result<Vec<WindowsRouteObservation>, String> {
        let mut table = std::ptr::null_mut();
        // SAFETY: fixed family, initialized out pointer; successful API provides a live allocated table.
        let code = unsafe {
            GetIpForwardTable2(
                if family == AddressFamily::V4 {
                    AF_INET
                } else {
                    AF_INET6
                },
                &mut table,
            )
        };
        if code.0 != 0 {
            return Err(api_error("GetIpForwardTable2", code.0));
        }
        if table.is_null() {
            return Err("IP Helper returned null table".into());
        }
        let owner = MibOwner(table);
        // SAFETY: native successful table owns this header; read only the header before count validation.
        let count = unsafe { std::ptr::addr_of!((*owner.0).NumEntries).read() } as usize;
        if count > MAX_ROUTE_ROWS {
            return Err("native route count limit exceeded".into());
        }
        // SAFETY: API supplies count valid POD rows; cap is checked before constructing the slice.
        let native = unsafe {
            std::slice::from_raw_parts(
                std::ptr::addr_of!((*owner.0).Table).cast::<MIB_IPFORWARD_ROW2>(),
                count,
            )
        };
        let mut out = Vec::new();
        for row in native {
            let (actual, address, _) = socket_bits(row.DestinationPrefix.Prefix)?;
            if actual != family {
                return Err("native route query family mismatch".into());
            }
            let next = socket_bits(row.NextHop).and_then(|(f, a, scope)| {
                if f == family {
                    Ok((a, scope))
                } else {
                    Err("native next-hop family mismatch".into())
                }
            });
            // SAFETY: documented NET_LUID POD value in this successfully enumerated row.
            let luid = unsafe { row.InterfaceLuid.Value };
            out.push(WindowsRouteObservation {
                interface: WindowsInterfaceRef {
                    alias: missing("route alias not observed"),
                    luid: Fact::Known(luid),
                    if_index: Fact::Known(row.InterfaceIndex),
                },
                family,
                prefix: format!("{address}/{}", row.DestinationPrefix.PrefixLength),
                next_hop: next
                    .as_ref()
                    .map(|(a, _)| Fact::Known(*a))
                    .unwrap_or_else(|e| Fact::Unknown(e.clone())),
                next_hop_scope_id: next.map(|(_, scope)| scope).unwrap_or_else(Fact::Unknown),
                route_metric: if row.Metric == u32::MAX {
                    missing("route metric unused sentinel")
                } else {
                    Fact::Known(row.Metric)
                },
                interface_metric: missing("interface metric not observed in this source"),
            });
        }
        Ok(out)
    };
    operation().map(observed).unwrap_or_else(Fact::Unknown)
}
// Packed fields are copied by value. Never read connection logon LUID as NET_LUID.
fn ras_row(conn: RASCONNW) -> Result<WindowsRasObservation, String> {
    if conn.dwSize != size_of::<RASCONNW>() as u32 {
        return Err("RAS record length unsupported".into());
    }
    let raw: [u16; 257] = conn.szEntryName;
    let flags = conn.dwFlags;
    let name = raw
        .iter()
        .position(|x| *x == 0)
        .and_then(|end| String::from_utf16(&raw[..end]).ok())
        .map(|s| private_ras_name(&s))
        .unwrap_or_else(|| missing("RAS entry name unavailable"));
    Ok(WindowsRasObservation {
        name,
        all_users: flags & RASCF_AllUsers != 0,
        interface: missing("RAS interface association not established"),
    })
}

#[allow(
    unsafe_code,
    reason = "fixed readonly RAS enumeration into capped packed POD records; names sanitized before DTO"
)]
pub(super) fn ras() -> Fact<ReadRows<WindowsRasObservation>> {
    let operation = || -> Result<Vec<WindowsRasObservation>, String> {
        let size = size_of::<RASCONNW>();
        let mut wanted = 1usize;
        for _ in 0..3 {
            if wanted == 0 || wanted > MAX_INTERFACE_ROWS {
                return Err("RAS native count limit exceeded".into());
            }
            let mut records = vec![RASCONNW::default(); wanted];
            records[0].dwSize = size as u32;
            let mut bytes = (wanted * size) as u32;
            let mut count = 0u32;
            // SAFETY: live capped writable POD array, first dwSize identifies its ABI version.
            let code =
                unsafe { RasEnumConnectionsW(Some(records.as_mut_ptr()), &mut bytes, &mut count) };
            if code == 603 {
                wanted = (bytes as usize).div_ceil(size);
                continue;
            }
            if code != 0 {
                return Err(api_error("RasEnumConnectionsW", code));
            }
            if count as usize > records.len() || bytes as usize > records.len() * size {
                return Err("RAS count or length beyond capacity".into());
            }
            let mut out = Vec::new();
            for conn in records.into_iter().take(count as usize) {
                if conn.dwSize != size as u32 {
                    return Err("RAS record length unsupported".into());
                }
                out.push(ras_row(conn)?);
            }
            return Ok(out);
        }
        Err("RAS buffer growth retry limit exceeded".into())
    };
    operation().map(observed).unwrap_or_else(Fact::Unknown)
}

#[cfg(test)]
mod tests;

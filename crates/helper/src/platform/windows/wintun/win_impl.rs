use super::super::netinfo::u64_cells_for;
use super::{adapter_name_is_probeable, ipv4_s_addr, AdapterProbe};
use core::ffi::c_void;
use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceIndexToLuid, ConvertInterfaceLuidToAlias, GetAdaptersAddresses,
    GetBestInterfaceEx,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    GAA_FLAG_SKIP_UNICAST, IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::NetworkManagement::Ndis::{IF_MAX_STRING_SIZE, NET_LUID_LH};
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, IN6_ADDR, IN6_ADDR_0, IN_ADDR, IN_ADDR_0, SOCKADDR, SOCKADDR_IN,
    SOCKADDR_IN6, SOCKADDR_IN6_0,
};

/// 查询 Windows 路由表对目标 IPv4 选出的最佳接口索引。
///
/// 与 PowerShell `Find-NetRoute -RemoteIPAddress ...` 回答同一个问题，但直接调用 IP Helper API：
/// 不创建 PowerShell/.NET 进程，也不解析本地化文本。调用者只比较起核前后的索引是否变化，故无需
/// 再把索引反查成可变、可本地化的 FriendlyName。
pub fn best_route_interface_index(ip: std::net::Ipv4Addr) -> std::io::Result<u32> {
    let address = SOCKADDR_IN {
        sin_family: AF_INET,
        sin_port: 0,
        sin_addr: IN_ADDR {
            S_un: IN_ADDR_0 {
                S_addr: ipv4_s_addr(ip),
            },
        },
        sin_zero: [0; 8],
    };
    let mut index = 0u32;
    // SAFETY: `address` 是本栈帧内完整初始化的 IPv4 SOCKADDR，指针仅在同步调用期间借用；
    // `index` 是有效可写的 u32。GetBestInterfaceEx 不保留二者指针。
    let rc = unsafe { GetBestInterfaceEx((&raw const address).cast::<SOCKADDR>(), &raw mut index) };
    if rc != NO_ERROR {
        return Err(std::io::Error::from_raw_os_error(rc as i32));
    }
    if index == 0 {
        return Err(std::io::Error::other(
            "GetBestInterfaceEx returned interface index 0",
        ));
    }
    Ok(index)
}

/// 查询任意 IP 目的的最佳接口，并返回 sing-box `bind_interface` 接受的 InterfaceAlias。
/// 全程使用 IP Helper API，不冷启 PowerShell，也不解析本地化命令输出。
pub fn best_route_interface_alias(ip: std::net::IpAddr) -> std::io::Result<String> {
    let index = match ip {
        std::net::IpAddr::V4(ip) => best_route_interface_index(ip)?,
        std::net::IpAddr::V6(ip) => {
            let address = SOCKADDR_IN6 {
                sin6_family: AF_INET6,
                sin6_port: 0,
                sin6_flowinfo: 0,
                sin6_addr: IN6_ADDR {
                    u: IN6_ADDR_0 { Byte: ip.octets() },
                },
                Anonymous: SOCKADDR_IN6_0 { sin6_scope_id: 0 },
            };
            let mut index = 0u32;
            // SAFETY: `address` 与输出 index 都完整初始化，指针只在同步调用期间借用。
            let rc = unsafe {
                GetBestInterfaceEx((&raw const address).cast::<SOCKADDR>(), &raw mut index)
            };
            if rc != NO_ERROR {
                return Err(std::io::Error::from_raw_os_error(rc as i32));
            }
            if index == 0 {
                return Err(std::io::Error::other(
                    "GetBestInterfaceEx returned interface index 0",
                ));
            }
            index
        }
    };

    let mut luid = NET_LUID_LH::default();
    // SAFETY: `luid` 是有效可写结构，API 不保留指针。
    let rc = unsafe { ConvertInterfaceIndexToLuid(index, &raw mut luid) };
    if rc != NO_ERROR {
        return Err(std::io::Error::from_raw_os_error(rc as i32));
    }
    let mut alias = [0u16; IF_MAX_STRING_SIZE as usize + 1];
    // SAFETY: alias 缓冲区长度按 API 上限分配，luid/alias 指针仅在同步调用期间借用。
    let rc =
        unsafe { ConvertInterfaceLuidToAlias(&raw const luid, alias.as_mut_ptr(), alias.len()) };
    if rc != NO_ERROR {
        return Err(std::io::Error::from_raw_os_error(rc as i32));
    }
    let len = alias
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(alias.len());
    let alias = String::from_utf16(&alias[..len])
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if alias.trim().is_empty() {
        return Err(std::io::Error::other(
            "ConvertInterfaceLuidToAlias returned an empty alias",
        ));
    }
    Ok(alias)
}

/// 承载缓冲区用 `Vec<u64>` 的**前提**：目标结构体的 align 不得超过 u64 的 align。
/// 与 [`super::super::netinfo`] 同款断言、同款承载类型（两处对称，别一处 u8 一处 u64）。
const _: () = assert!(
    std::mem::align_of::<IP_ADAPTER_ADDRESSES_LH>() <= std::mem::align_of::<u64>(),
    "IP_ADAPTER_ADDRESSES_LH 的对齐已超过 u64，Vec<u64> 承载不再安全"
);

/// Windows IP Helper 适配器枚举实现（`GetAdaptersAddresses`）。
///
/// 跳过所有不需要的字段（只取 FriendlyName，按前缀匹配）。比 Go 源无对应（Go 用 sing-box 自管 wintun，
/// 本 Rust crate 主动加这层探测以补维度7 #30）。
pub struct WinAdapterProbe;

impl AdapterProbe for WinAdapterProbe {
    fn list_matching_adapters(&self) -> std::io::Result<Vec<String>> {
        list_matching_adapters_with(|flags, addresses, size| {
            // SAFETY: the enumerator owns an aligned buffer of at least *size
            // bytes (or a null sizing probe). The API retains no pointers.
            unsafe { GetAdaptersAddresses(0, flags, std::ptr::null(), addresses, size) }
        })
    }
}

// A narrow FFI seam keeps sizing, requested fields and name filtering on the
// production path under test; mocks must populate the same aligned buffer.
fn list_matching_adapters_with(
    mut enumerate: impl FnMut(u32, *mut IP_ADAPTER_ADDRESSES_LH, &mut u32) -> u32,
) -> std::io::Result<Vec<String>> {
    // SAFETY: GetAdaptersAddresses 是线程安全的 Win32 API。我们传 SizePointer 控制缓冲区，
    // 重试直到 ERROR_BUFFER_OVERFLOW 不再出现（标准两步法：先取大小，再取内容）。
    // 缓冲区由我们拥有，释放由 Vec drop 处理。指针仅在本函数作用域内有效。
    let flags: u32 = GAA_FLAG_SKIP_UNICAST
        | GAA_FLAG_SKIP_ANYCAST
        | GAA_FLAG_SKIP_MULTICAST
        | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 0;
    let mut retries = 0u32;
    loop {
        // SAFETY: 第一次调用取所需大小（pAdapterAddresses=NULL），返回 ERROR_BUFFER_OVERFLOW。
        // 详见上方 SAFETY 注释。
        let rc = enumerate(flags, std::ptr::null_mut(), &mut size);
        if rc == ERROR_BUFFER_OVERFLOW {
            break;
        }
        if rc == NO_ERROR && size == 0 {
            return Ok(Vec::new()); // 无适配器
        }
        if rc != NO_ERROR {
            return Err(std::io::Error::from_raw_os_error(rc as i32));
        }
        retries += 1;
        if retries > 3 {
            return Err(std::io::Error::other("GetAdaptersAddresses size loop"));
        }
    }
    // 缓冲区按 u64 槽分配：结果要按 `&IP_ADAPTER_ADDRESSES_LH` 解引用，该结构体含指针/u64
    // ⇒ align = 8，而 `Vec<u8>` 只保证 align 1，从未对齐地址造引用按语言规则即 UB
    //（实践上 Windows 堆恰好 16 字节对齐所以不炸，但那是靠分配器实现细节而非语言保证）。
    let mut buf: Vec<u64> = vec![0u64; u64_cells_for(size)];
    // SAFETY: buf 容量 ≥ size 字节（u64_cells_for 向上取整）且对齐满足目标结构体（上方 const
    // 断言编译期保证）。pAdapterAddresses 指向 buf 首字节。后续遍历以 SingleLinkedList 形式
    //（Next 指针），仅读不写。
    let rc = enumerate(
        flags,
        buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
        &mut size,
    );
    if rc != NO_ERROR {
        return Err(std::io::Error::from_raw_os_error(rc as i32));
    }
    // 遍历链表，取 FriendlyName 匹配前缀。
    let mut out = Vec::new();
    let mut ptr = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    // SAFETY: 链表节点由 GetAdaptersAddresses 填充，Next 为 NULL 终止。ptr 非空时解引用安全。
    while !ptr.is_null() {
        let entry: &IP_ADAPTER_ADDRESSES_LH = unsafe { &*ptr };
        if let Some(name) = read_friendly_name(entry) {
            // 前缀判据经 `adapter_name_is_probeable` 收口 —— 存在性探测的调用方要用**同一个**
            // 谓词判「这个名字我方看不看得见」，两处各写一遍 `starts_with` 就是第二真值源。
            if adapter_name_is_probeable(&name) {
                out.push(name);
            }
        }
        ptr = entry.Next as *const IP_ADAPTER_ADDRESSES_LH;
    }
    Ok(out)
}

/// 读 IP_ADAPTER_ADDRESSES_LH.FriendlyName（UTF-16 widestring → String）。
///
/// SAFETY: 调用方保证 entry 有效；FriendlyName 指向 widestring（NULL 终止）。
fn read_friendly_name(entry: &IP_ADAPTER_ADDRESSES_LH) -> Option<String> {
    let p = entry.FriendlyName;
    if p.is_null() {
        return None;
    }
    // SAFETY: FriendlyName 是 widestring（NULL 终止），由 GetAdaptersAddresses 填充，生命周期同 entry。
    let mut len = 0usize;
    while unsafe { *p.add(len) } != 0 {
        len += 1;
        if len > 256 {
            return None; // 防越界（适配器名不会这么长）
        }
    }
    // SAFETY: p 指向 len 个有效 u16（widestring），由 GetAdaptersAddresses 填充。
    let slice: &[u16] = unsafe { std::slice::from_raw_parts(p, len) };
    String::from_utf16(slice).ok()
}

// 抑制未用警告（c_void 在签名里出现但本模块未直接命名使用）。
const _: Option<*const c_void> = None;

#[cfg(test)]
mod tests;

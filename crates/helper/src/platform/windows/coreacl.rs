//! 受保护内核目录的 ACL / owner **判据**（纯逻辑，无 cfg，Linux 单测；spec §3.4 · Q9）。
//!
//! ## 定位（不得拔高）
//!
//! 这是与 mac/linux 拉平的 chokepoint + **纵深**，**不是「消除提权面」**：helper.exe 本体仍源自
//! 用户可写域（面 K 仍开着）⇒ 对**同账户**攻击者的边际收益 ≈ 0。它买到的是：核此后住在
//! 「SYSTEM 写、普通用户只读」的目录里，且这个前提每次起核前被复核一次 —— 安装脚本设的 ACL
//! 被谁（管理员手动、第三方安装器、备份还原、`icacls /reset`）放宽了，起核会停下来说出来，
//! 而不是照旧 exec 一个任何人都能改的二进制。
//!
//! ## 为什么判据在这里、不在 FFI 腿里
//!
//! 读 DACL 只能在 Windows 跑（`GetNamedSecurityInfoW`），而「哪些 SID / 哪些位算可接受」是这批
//! 里唯一有安全后果的逻辑。把两者分开：本模块吃**已搬运成纯数据**的 owner SID 串 + ACE 列表，
//! 出 verdict，Linux 上逐形态单测；[`super::winproc`] 那侧只搬运不判断。
//!
//! ## 白名单，不是黑名单（spec 必改2）
//!
//! 判「有没有非特权主体能写」用**白名单**：任何授写类权的 ACE，其 SID 不在
//! {[`SID_LOCAL_SYSTEM`], [`SID_ADMINISTRATORS`]} 里即判放宽。黑名单（枚举「不许出现的 SID」）
//! 会漏 `INTERACTIVE`（S-1-5-4）、`CREATOR OWNER`（S-1-3-0）、以及任何一个具体用户 SID ——
//! 那些恰恰是「用户把自己加进去」后最常见的形态。
//!
//! ## 正面断言（缺了同样判异常）
//!
//! 只写「不许出现 X」会被「什么都没发生」骗过：一个 0 条 ACE 的 DACL（= 拒绝所有人）满足
//! 「没有非特权主体能写」，却让 helper 自己 install-core 也写不进去。故 SYSTEM 与
//! Administrators **各自**必须拿到写类权，缺失也是异常。
//!
//! ## 三个必须分开处理的陷阱
//!
//! 1. **NULL DACL ≠ 空 DACL**。NULL DACL 的 Win32 语义是「所有人完全访问」（最坏情形）；
//!    非 NULL 但 0 条 ACE 是「拒绝所有人」。折成一个分支 = 把最危险的那个形态和一个
//!    功能性故障混为一谈，拿到 `coredir-acl-weakened` 的人无从分辨。
//! 2. **`INHERIT_ONLY_ACE` 的 ACE 对对象自身不生效**（只往下继承）。评估「**这个对象**被谁授了
//!    写」时必须排除它，否则把「只给子项的授权」误当成对本目录的授权 ⇒ 假红、拒起核。
//! 3. **`ACCESS_DENIED` 类 ACE 不是授予**。算进白名单违规 = 把「管理员显式拒了某个用户」
//!    读成「某个用户能写」（假红）；但**针对 SYSTEM/Administrators 写类权的 DENY 会让 helper
//!    写不进核** ⇒ 那是异常，不能视而不见。（Windows 的 DENY/ALLOW 优先序细节不在本判据射程内
//!    —— 本模块只做「不把 DENY 当 ALLOW」+「不对特权侧的 DENY 装瞎」。）

use std::fmt;

/// `NT AUTHORITY\SYSTEM` 的 well-known SID（机器无关字面量）。
pub const SID_LOCAL_SYSTEM: &str = "S-1-5-18";

/// `BUILTIN\Administrators` 的 well-known SID（机器无关字面量）。
pub const SID_ADMINISTRATORS: &str = "S-1-5-32-544";

/// 允许持有写类权的主体（白名单）。
///
/// 两个都是 well-known SID，**跨机器恒等**（不含域/机器 RID）⇒ 直接按字符串比对即可，
/// 不必在 FFI 腿里 `CreateWellKnownSid` + `EqualSid` 现造再比。
pub const PRIVILEGED_SIDS: [&str; 2] = [SID_LOCAL_SYSTEM, SID_ADMINISTRATORS];

/// 配套 DLL 名（与核同目录，兜底白名单的第二个文件）。
///
/// **真值住在 [`crate::core_install`]，本处只是转发**：`platform::windows` 的 cfg 谓词里那个
/// `test` 只在 polaris-helper 自己的 test 编译期成立，src-tauri 的跨 crate 等值门够不着这里。
/// 理由全文见 [`crate::core_install::CRONET_DLL_NAME_WIN`]。
pub use crate::core_install::CRONET_DLL_NAME_WIN;

/// 「写类权」位掩码。任一位命中即视为可改该对象。
///
/// 位值逐条取自 `windows-sys`（`Win32::Storage::FileSystem` / `Win32::Foundation`），在
/// [`super::winproc`] 里有 **windows-only 编译期断言**钉住「本常量 == 那些常量的并」——
/// 一动就编不过，且 CI 的 msvc 交叉 clippy 会跑到。位值必须写在这里的理由同
/// [`super::logic::pipe_open_mode`]：本模块在 Linux 上也编译（判据要有门可跑），而
/// `windows-sys` 是 `[target.'cfg(windows)'.dependencies]`，Linux 上不在依赖图里。
///
/// **`FILE_ALL_ACCESS`（icacls `(F)`）刻意不并进来**：它是复合权，值 `0x001F_01FF` 里含
/// `READ_CONTROL`(0x2_0000) 与 `SYNCHRONIZE`(0x10_0000) —— 而 `Users:(RX)`
/// （`FILE_GENERIC_READ|FILE_GENERIC_EXECUTE` = `0x12_00A9`）同样含这两位。把 `(F)` 整值
/// 并进掩码，`(RX)` 就会被判成写类 ⇒ spec §3.4 要求放行的通过态（`Users:(RX)`）直接变假红。
/// 不并它也**不漏** `(F)`：`FILE_ALL_ACCESS` 本身含 `FILE_WRITE_DATA|…|WRITE_OWNER`，下面这些
/// 原子位必命中。这一条同样由 winproc 那侧的两条编译期断言钉住（`(F)` 必命中、`(RX)` 必不命中）。
///
/// **`FILE_DELETE_CHILD`（0x40）是写类权，不是「目录遍历」那类无害位**：它允许删除/改名目录下的
/// 子项**而绕过子项自己的 DACL**。`icacls <dir> /grant "*S-1-5-32-545:(DC)"` 之后普通用户可随时
/// 删掉 `sing-box.exe` ⇒ helper 的 `CreateProcessW` 恒失败 = 永久 DoS，而自检报「一切正常」。
pub const WRITE_ACCESS_MASK: u32 = 0x0000_0002 // FILE_WRITE_DATA
    | 0x0000_0004 // FILE_APPEND_DATA
    | 0x0000_0010 // FILE_WRITE_EA
    | 0x0000_0040 // FILE_DELETE_CHILD
    | 0x0000_0100 // FILE_WRITE_ATTRIBUTES
    | 0x0001_0000 // DELETE
    | 0x0004_0000 // WRITE_DAC
    | 0x0008_0000 // WRITE_OWNER
    | 0x1000_0000 // GENERIC_ALL
    | 0x4000_0000; // GENERIC_WRITE

/// 「读执行类权」位掩码（icacls `(RX)` 覆盖的原子权）。
///
/// **只服务 warn 级自曝，不进任何拒绝判据** —— 见 [`grants_non_privileged_read_execute`]。位值同样
/// 由 [`super::winproc`] 的 windows-only 编译期断言钉住（理由同 [`WRITE_ACCESS_MASK`]）。
pub const READ_EXECUTE_ACCESS_MASK: u32 = 0x0000_0001 // FILE_READ_DATA
    | 0x0000_0020 // FILE_EXECUTE
    | 0x1000_0000 // GENERIC_ALL
    | 0x2000_0000 // GENERIC_EXECUTE
    | 0x8000_0000; // GENERIC_READ

/// `INHERIT_ONLY_ACE`（`Win32::Security` 同名常量，winproc 侧编译期断言钉住）。
///
/// 带此标志的 ACE **对对象自身不生效**，只参与向子项继承。
pub const ACE_FLAG_INHERIT_ONLY: u32 = 0x0000_0008;

/// 搬运腿读 ACL/ACE **结构本身**失败时填的哨兵 `AceType`（`AceKind::Unparsed` 的第三个成因）。
///
/// Win32 定义的 AceType 只用到 `0x00..=0x15`，`0xFF` 不与任何真实类型撞号 ⇒ 真机上看到
/// `AceType=255` 即可一眼分辨「这条不是某种没见过的 ACE，是 `GetAce`/`GetAclInformation` 自己失败」。
pub const ACE_TYPE_UNREADABLE: u8 = 0xFF;

/// 一条 ACE 的类型（DACL 里只会出现 allow / deny 两族）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AceKind {
    /// `ACCESS_ALLOWED_ACE_TYPE`（0）—— 授予。
    Allow,
    /// `ACCESS_DENIED_ACE_TYPE`（1）—— 拒绝。**不构成授予**。
    Deny,
    /// **判不了的 ACE**，携原始 `AceType`（读结构本身失败时携 [`ACE_TYPE_UNREADABLE`]）。三个成因：
    /// 1. **object ACE**（type 5/6/7 等）—— 头部多一个 `Flags` + 最多两个 GUID，`SidStart` 偏移与
    ///    `ACCESS_ALLOWED_ACE` **不同**，按那个布局读出来的 SID 是垃圾，故一律不报；
    /// 2. **callback / conditional ACE**（type 9 / 0xA）—— 布局与 `ACCESS_ALLOWED_ACE`
    ///    **逐字节相同**（`{ACE_HEADER, ACCESS_MASK, DWORD SidStart}`），条件表达式挂在 SID 之后。
    ///    即：SID 与 mask 其实读得出来，但本批**不解析其条件**（条件为假时该 ACE 不生效，按 mask
    ///    直接判会假红），故一并按不可判读处理 —— 失败向关；
    /// 3. 类型认得、但 SID 转串失败 / `GetAce`·`GetAclInformation` 自身失败 —— 读到了这条 ACE
    ///    （或这份 DACL），却说不出它授给谁。
    ///
    /// 判据按**失败向关**处理：DACL 里有一条读不懂的条目，就无法证明它不是一条授予，故判异常。
    /// 安装脚本的 `/inheritance:r` + `/grant:r` 会把 DACL 整份替换成三条 plain ACE，目标态下本分支
    /// 不该出现。
    ///
    /// **企业 Central Access Policy 不会误伤这条**：CAP 经 **SACL** 的 `SYSTEM_SCOPED_POLICY_ID_ACE`
    /// 生效，而搬运腿只请求 `OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION`，SACL 压根
    /// 不读（读它还需 `SE_SECURITY_NAME` 特权）⇒ 域里挂了 CAP 的机器在这里与没挂 CAP 的机器同形。
    Unparsed(u8),
}

/// 一条已搬运成纯数据的 ACE。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    /// 受托主体的 SID 串（`ConvertSidToStringSidW` 的结果）。[`AceKind::Unparsed`] 时为空串。
    pub sid: String,
    /// 访问掩码。[`AceKind::Unparsed`] 时为 0。
    pub mask: u32,
    /// ACE 类型。
    pub kind: AceKind,
    /// `ACE_HEADER.AceFlags`。本判据只消费 [`ACE_FLAG_INHERIT_ONLY`]（该字段在所有 ACE
    /// 类型里偏移相同，故 [`AceKind::Unparsed`] 也读得到）。
    pub flags: u32,
}

impl Ace {
    /// 「这条 ACE（或这份 DACL）判不了」的哨兵条目 —— 搬运腿读 ACL/ACE **结构本身**失败时填它。
    ///
    /// **住在这里、不住在 [`super::winproc`]**：那边是 `#[cfg(windows)]`，本机跑不到；而这个构造
    /// 恰恰有一个必须被门钉住的语义 —— `flags` 必须**不含** [`ACE_FLAG_INHERIT_ONLY`]，否则它对
    /// 对象自身不生效 ⇒ 判据直接跳过 ⇒ 「读不出来的 ACE」静默变成「没有这条 ACE」，失败向开。
    #[must_use]
    pub fn unreadable() -> Self {
        Self {
            sid: String::new(),
            mask: 0,
            kind: AceKind::Unparsed(ACE_TYPE_UNREADABLE),
            flags: 0,
        }
    }

    /// 本 ACE 对**对象自身**是否生效（`INHERIT_ONLY` 的只给子项）。
    #[must_use]
    pub const fn is_effective_on_object(&self) -> bool {
        self.flags & ACE_FLAG_INHERIT_ONLY == 0
    }

    /// 本 ACE 是否碰到写类权。
    #[must_use]
    pub const fn touches_write(&self) -> bool {
        self.mask & WRITE_ACCESS_MASK != 0
    }
}

/// 一个对象（目录或文件）的 owner + DACL，已搬运成纯数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectSecurity {
    /// owner 的 SID 串；安全描述符里**没有属主**时为 `None`（异常，不是「读失败」）。
    pub owner_sid: Option<String>,
    /// DACL。
    ///
    /// - `None` = **NULL DACL**（Win32 语义：所有人完全访问）。
    /// - `Some(vec![])` = 非 NULL 的**空 DACL**（Win32 语义：拒绝所有人）。
    ///
    /// 两者语义相反，故不能用同一个表示（见模块文档陷阱 1）。
    pub dacl: Option<Vec<Ace>>,
}

/// 一条自检发现（**必须带上是哪个路径、哪条 ACE/owner 触发的** —— 否则真机上看到
/// `ERR coredir-acl-weakened` 无从定位）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclFinding {
    /// 触发的对象路径。
    pub path: String,
    /// 触发的具体判据。
    pub issue: AclIssue,
}

/// 具体判据分类。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AclIssue {
    /// 拿不到属主 SID：安全描述符里**没有**属主，或有属主但 SID 判读不了。
    ///
    /// 两个成因合并成一格（都不可能出现在目标态里，而分开只会多一个永不触发的分支）；
    /// 两者的处置必须相同 —— 「判不出属主」与「属主不对」同向，否则就是一个失败向开的洞。
    OwnerMissing,
    /// owner 不属于 {SYSTEM, Administrators}。
    OwnerNotPrivileged {
        /// 实读到的 owner SID。
        owner: String,
    },
    /// NULL DACL（所有人完全访问）。
    NullDacl,
    /// 非特权 SID 被授写类权（白名单违规）。
    WritableBySid {
        /// 被授权的主体。
        sid: String,
        /// 该 ACE 的访问掩码。
        mask: u32,
    },
    /// 特权 SID 的写类权被 DENY ACE 拒掉（helper 自己装不进核）。
    WriteDeniedForPrivileged {
        /// 被拒的主体。
        sid: String,
        /// 该 DENY ACE 的访问掩码。
        mask: u32,
    },
    /// DACL 里有一条布局判读不了的 ACE（见 [`AceKind::Unparsed`]）。
    UnparsedAce {
        /// 原始 `ACE_HEADER.AceType`。
        ace_type: u8,
    },
    /// 特权 SID 没有写类权（正面断言失败）。
    MissingWriteFor {
        /// 缺写类权的主体。
        sid: String,
    },
}

impl fmt::Display for AclIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OwnerMissing => f.write_str("拿不到属主 SID（无属主或 SID 判读不了）"),
            Self::OwnerNotPrivileged { owner } => {
                write!(f, "owner={owner} 不在 SYSTEM/Administrators 内")
            }
            Self::NullDacl => f.write_str("DACL 为 NULL（语义=所有人完全访问）"),
            Self::WritableBySid { sid, mask } => {
                write!(f, "非特权 SID {sid} 被授写类权 mask=0x{mask:08x}")
            }
            Self::WriteDeniedForPrivileged { sid, mask } => {
                write!(f, "{sid} 的写类权被 DENY ACE 拒绝 mask=0x{mask:08x}")
            }
            Self::UnparsedAce { ace_type } => {
                write!(f, "DACL 含布局不可判读的 ACE AceType={ace_type}")
            }
            Self::MissingWriteFor { sid } => write!(f, "{sid} 无写类权"),
        }
    }
}

impl fmt::Display for AclFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.issue)
    }
}

/// 一个对象读不到时的记录（Q9：**读不到 ≠ 被放宽**，两者不进同一个篮子）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableObject {
    /// 读不到的对象路径。
    pub path: String,
    /// 搬运腿给的原因（FFI 错误串）。
    pub error: String,
}

/// 整个受保护核目录的自检结论。
///
/// **两个篮子刻意分开**（Q9 拍板）：`findings` 非空 ⇒ 拒起核；`unreadable` 非空 ⇒ warn 继续。
/// 合并成一个「有问题」布尔量，就等于把「读不到」当成「被放宽」—— 一次 FFI 失败会让用户
/// 连不上网，而那台机器的 ACL 可能完全正常。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoreAclOutcome {
    /// 判定为放宽/异常的发现（空 = 通过）。
    pub findings: Vec<AclFinding>,
    /// 读不到的对象。
    pub unreadable: Vec<UnreadableObject>,
}

impl CoreAclOutcome {
    /// 是否判定为放宽/异常（⇒ 拒起核）。
    #[must_use]
    pub fn is_weakened(&self) -> bool {
        !self.findings.is_empty()
    }

    /// 放宽发现汇成单行 detail（wire 是行协议，**不得含换行**；Windows 路径也不可能含换行）。
    #[must_use]
    pub fn findings_detail(&self) -> String {
        self.findings
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// 读不到的对象汇成单行 detail（进 warn 日志）。
    #[must_use]
    pub fn unreadable_detail(&self) -> String {
        self.unreadable
            .iter()
            .map(|u| format!("{}: {}", u.path, u.error))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// 判一个对象（目录或文件）的 owner + DACL（spec §3.4 逐条）。
#[must_use]
pub fn judge_object(path: &str, sec: &ObjectSecurity) -> Vec<AclFinding> {
    let mut out = Vec::new();
    let mut add = |issue: AclIssue| {
        out.push(AclFinding {
            path: path.to_owned(),
            issue,
        });
    };

    // ===== owner 判定 =====
    match sec.owner_sid.as_deref() {
        None => add(AclIssue::OwnerMissing),
        Some(owner) if !is_privileged_sid(owner) => add(AclIssue::OwnerNotPrivileged {
            owner: owner.to_owned(),
        }),
        Some(_) => {}
    }

    // ===== DACL 判定 =====
    let Some(aces) = sec.dacl.as_ref() else {
        // 陷阱 1 的第一半：NULL DACL = 所有人完全访问。**不跑正面断言** —— 这个形态下
        // SYSTEM/Administrators 确实有写类权，跑了只会得出「正面断言通过」的误导结论。
        add(AclIssue::NullDacl);
        return out;
    };

    // 正面断言的记账：两个特权 SID 各自是否拿到了生效的 ALLOW 写类权。
    let mut privileged_has_write = [false; PRIVILEGED_SIDS.len()];
    for ace in aces {
        // 陷阱 2：INHERIT_ONLY 的 ACE 对本对象不生效 —— 既不算授权，也不算正面断言的证据。
        if !ace.is_effective_on_object() {
            continue;
        }
        if let AceKind::Unparsed(ace_type) = ace.kind {
            // 布局读不懂 ⇒ 无法证明它不是一条授予（失败向关）。
            add(AclIssue::UnparsedAce { ace_type });
            continue;
        }
        if !ace.touches_write() {
            continue; // `Users:(RX)` 这类只读执行：放行，也不构成写类权证据。
        }
        let privileged_idx = PRIVILEGED_SIDS.iter().position(|s| *s == ace.sid);
        match (ace.kind, privileged_idx) {
            // 白名单违规：任何非特权主体被**授**写类权。
            (AceKind::Allow, None) => add(AclIssue::WritableBySid {
                sid: ace.sid.clone(),
                mask: ace.mask,
            }),
            // 特权主体被授写类权 = 正面断言的证据。
            (AceKind::Allow, Some(i)) => privileged_has_write[i] = true,
            // 陷阱 3：针对特权主体写类权的 DENY ⇒ helper 自己装不进核，是异常。
            (AceKind::Deny, Some(_)) => add(AclIssue::WriteDeniedForPrivileged {
                sid: ace.sid.clone(),
                mask: ace.mask,
            }),
            // 非特权主体的 DENY 是收紧，不是授予 —— 不算违规。
            (AceKind::Deny, None) => {}
            // 上面已 continue 掉。
            (AceKind::Unparsed(_), _) => {}
        }
    }

    // ===== 正面断言 =====
    // 空 DACL（0 条 ACE，语义「拒绝所有人」）在此落地为「两个特权 SID 都缺写类权」——
    // 与 NULL DACL 走的是**不同分支、不同 findings**（陷阱 1 的第二半）。
    for (i, sid) in PRIVILEGED_SIDS.iter().enumerate() {
        if !privileged_has_write[i] {
            add(AclIssue::MissingWriteFor {
                sid: (*sid).to_owned(),
            });
        }
    }
    out
}

/// 判整组对象（目录 + 每个白名单文件），并按 Q9 把「放宽」与「读不到」分进两个篮子。
///
/// **覆盖面是目录 + 每个文件**：目录锁得住、文件被单独 `/grant` 放宽，照样是可注入的核
/// —— 只判目录的门等于没门。
#[must_use]
pub fn judge_core_objects(objects: &[(String, Result<ObjectSecurity, String>)]) -> CoreAclOutcome {
    let mut outcome = CoreAclOutcome::default();
    for (path, read) in objects {
        match read {
            Ok(sec) => outcome.findings.extend(judge_object(path, sec)),
            Err(error) => outcome.unreadable.push(UnreadableObject {
                path: path.clone(),
                error: error.clone(),
            }),
        }
    }
    outcome
}

/// 自检取材面的**兜底**部分：**真正要被 exec 的那个目录** + 那个二进制 + 同目录的配套 DLL。
///
/// **这三个是兜底，不是全部**：真正的覆盖面由 [`extend_targets_with_dir_entries`] 用目录的
/// **实际条目**补齐（攻击者预创建的第四个文件不在任何白名单里，只硬编码两个名字 = 门有洞）。
/// 保留这两个白名单名的语义是「文件不存在也要问一次」：枚举腿失败时覆盖面不至于归零。
///
/// 取 `singbox_bin` 的父目录而非 `<support>\core`（主会话拍板 1）：install-core 的**目标**要
/// 不可注入（故由 support 派生），而自检必须守住**实际执行面** —— 存量安装的 SCM ImagePath
/// 还指着老路径时，按派生目录自检等于守了个空目录，而 exec 的是另一个谁都能写的文件。
///
/// 返回 `None` = 从 `singbox_bin` 推不出目录（空串 / 裸文件名）。那是配置异常，**不是**「被放宽」，
/// 调用方按 Q9 的读不到腿处理（warn 继续）。
///
/// 路径切分走 [`super::logic::filepath_dir`] 而不是 `Path::parent()`：`std::path` 在 Linux 上
/// **只认 `/`**，会把 `C:\Program Files\Polaris\sing-box.exe` 整段当成一个文件名 ⇒ 本机单测里
/// 取材面恒空（门恒绿）、生产上却正常 —— 这批全部测试都跑在 Linux 上。
#[must_use]
pub fn core_acl_targets(singbox_bin: &str) -> Option<Vec<String>> {
    let dir = super::logic::filepath_dir(singbox_bin)?;
    if dir.is_empty() {
        return None;
    }
    Some(vec![
        dir.to_owned(),
        singbox_bin.to_owned(),
        join_win(dir, CRONET_DLL_NAME_WIN),
    ])
}

/// 把目录的**实际条目**并进取材面（与 [`core_acl_targets`] 的兜底名去重）。
///
/// ## 为什么取材面不能是硬编码的两个文件
///
/// 全程 Medium IL、不需要提权的可达路径：攻击者在首装前预创建 `C:\ProgramData\Polaris\core`，
/// 在里面放一个文件，并给它设一个 **`SE_DACL_PROTECTED`（去继承）** 的 DACL 含 `Users:(F)`
/// —— 他是 owner，这步零特权。安装脚本跑：`New-Item -Force` 不动已有目录；`/setowner /T` 把
/// owner 收走（这条有效）；但 `/inheritance:r` + `/grant:r` 的**传播按定义跳过 protected DACL
/// 的子项** ⇒ 那个文件的 `Users:(F)` 原样留着。自检若只问三个硬编码对象 ⇒ 通过。
/// （prune 只在下次 install-core 且该名不在 `names` 里时才删，且 best-effort 静默忽略失败，兜不住。）
///
/// ## 去重是必须的，不是整洁
///
/// 枚举必然重新报出 `sing-box.exe` / `libcronet.dll`；不去重则同一对象被判两遍，findings 里出现
/// 重复条目 —— 真机上看到同一路径报两次，第一反应是「判据在循环」而不是「取材面有交集」。
/// 比对走 [`same_win_path`]（Windows 语义：大小写不敏感 + `/`≡`\`），不是裸字符串相等。
#[must_use]
pub fn extend_targets_with_dir_entries(
    targets: &[String],
    dir: &str,
    entries: &[String],
) -> Vec<String> {
    let mut out = targets.to_vec();
    for name in entries {
        let path = join_win(dir, name);
        if !out.iter().any(|p| same_win_path(p, &path)) {
            out.push(path);
        }
    }
    out
}

/// 该对象上**是否存在**给非特权主体的读执行权（安装脚本承诺的 `Users:(RX)`）。
///
/// **只产 warn，绝不拒起核** —— 这是「必改7 的整条因果链现在没有任何自曝腿」的那条腿：
/// `Users:(RX)` 掉了之后，app（Medium IL）读不到受保护目录里的 dest ⇒ `protected_core_path_in`
/// 算出的那个文件永远判「不存在」⇒ 每次起核白推 80MB，且内核自证恒告警。helper 修不了它
/// （改 ACL 是安装脚本的事），但报得出来；不报的话这条链在真机上完全静默。
///
/// NULL DACL 下所有人都有读执行权 ⇒ 返 `true`（该形态另有 [`AclIssue::NullDacl`] 管，不在本条重复报）。
#[must_use]
pub fn grants_non_privileged_read_execute(sec: &ObjectSecurity) -> bool {
    let Some(aces) = sec.dacl.as_ref() else {
        return true;
    };
    aces.iter().any(|ace| {
        ace.is_effective_on_object()
            && ace.kind == AceKind::Allow
            && !is_privileged_sid(&ace.sid)
            && ace.mask & READ_EXECUTE_ACCESS_MASK != 0
    })
}

/// Windows 路径拼接（`\` 分隔，已带尾分隔符的根形态如 `C:\` 不重复加）。
///
/// 同样不用 `Path::join` —— 它在 Linux 上拼 `/`，会让本机单测看到的路径与生产不同形。
#[must_use]
pub fn join_win(dir: &str, name: &str) -> String {
    if dir.ends_with('\\') || dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}\\{name}")
    }
}

/// 两个路径是否指同一处（Windows 语义：大小写不敏感 + `/` 与 `\` 等价 + 忽略尾分隔符）。
///
/// 只服务「实际执行面 != 派生的 `<support>\core`」那条 **warn**（不拒起核），故做的是
/// 字面归一化比对，不解析 junction / 8.3 短名 / 相对路径。
#[must_use]
pub fn same_win_path(a: &str, b: &str) -> bool {
    let (a, b) = (
        super::logic::normalize_path(a),
        super::logic::normalize_path(b),
    );
    a.eq_ignore_ascii_case(&b)
}

/// SID 串是否在白名单内。
#[must_use]
pub fn is_privileged_sid(sid: &str) -> bool {
    PRIVILEGED_SIDS.contains(&sid)
}

/// spec §3.4 要求放行的**目标态**夹具：owner=SYSTEM + `SYSTEM:(F)` + `Administrators:(F)`
/// + `Users:(RX)`。
///
/// 供 [`super::ops::MockProcOps`] 当默认返回值 + 各测试当基线（改一处形态即造出一个变体）。
/// 掩码取真实 icacls 产物的值：`(F)` = `FILE_ALL_ACCESS`(0x1F01FF)，`(RX)` =
/// `FILE_GENERIC_READ|FILE_GENERIC_EXECUTE`(0x1200A9)。
#[cfg(test)]
#[must_use]
pub fn locked_down_fixture() -> ObjectSecurity {
    ObjectSecurity {
        owner_sid: Some(SID_LOCAL_SYSTEM.to_owned()),
        dacl: Some(vec![
            Ace {
                sid: SID_LOCAL_SYSTEM.to_owned(),
                mask: 0x001F_01FF,
                kind: AceKind::Allow,
                flags: 0x3, // OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE（对本对象仍生效）
            },
            Ace {
                sid: SID_ADMINISTRATORS.to_owned(),
                mask: 0x001F_01FF,
                kind: AceKind::Allow,
                flags: 0x3,
            },
            Ace {
                sid: "S-1-5-32-545".to_owned(), // BUILTIN\Users
                mask: 0x0012_00A9,
                kind: AceKind::Allow,
                flags: 0x3,
            },
        ]),
    }
}

#[cfg(test)]
mod tests;

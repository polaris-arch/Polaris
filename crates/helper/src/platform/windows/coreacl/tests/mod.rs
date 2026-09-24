//! 受保护核目录 ACL 判据的形态表（纯逻辑，本机 Linux 全覆盖）。
//!
//! 夹具用**真实 SID 形态**：`S-1-5-18`(SYSTEM) / `S-1-5-32-544`(Administrators) /
//! `S-1-5-32-545`(Users) / `S-1-5-4`(INTERACTIVE) / `S-1-3-0`(CREATOR OWNER) /
//! `S-1-5-21-1-2-3-1001`（具体用户）。后三个正是黑名单会漏、白名单才能挡住的那批。

use super::*;

/// 具体用户 SID（machine RID 形态）。
const SID_A_USER: &str = "S-1-5-21-1-2-3-1001";
/// `NT AUTHORITY\INTERACTIVE`。
const SID_INTERACTIVE: &str = "S-1-5-4";
/// `CREATOR OWNER`。
const SID_CREATOR_OWNER: &str = "S-1-3-0";
/// `BUILTIN\Users`。
const SID_USERS: &str = "S-1-5-32-545";

/// icacls `(F)` 的掩码（`FILE_ALL_ACCESS`）。
const MASK_FULL: u32 = 0x001F_01FF;
/// icacls `(RX)` 的掩码（`FILE_GENERIC_READ|FILE_GENERIC_EXECUTE`）。
const MASK_READ_EXECUTE: u32 = 0x0012_00A9;
/// icacls `(M)` 的掩码。
///
/// 真实 `(M)` = `0x0013_01BF`：`FILE_GENERIC_READ|FILE_GENERIC_WRITE|FILE_GENERIC_EXECUTE|DELETE`
/// —— 那个 `DELETE`(0x1_0000) 正是 `(M)` 与 `(RX)+写` 的分界。此前写成 `0x0012_01BF`（少 DELETE）
/// 不影响任何断言的红绿，但 §6.2 第 17 项要拿**真机 icacls 产物**回灌夹具，基线错了就对不上账。
const MASK_MODIFY: u32 = 0x0013_01BF;

fn allow(sid: &str, mask: u32) -> Ace {
    Ace {
        sid: sid.to_owned(),
        mask,
        kind: AceKind::Allow,
        flags: 0x3,
    }
}

fn deny(sid: &str, mask: u32) -> Ace {
    Ace {
        sid: sid.to_owned(),
        mask,
        kind: AceKind::Deny,
        flags: 0x3,
    }
}

const PATH_DIR: &str = r"C:\ProgramData\Polaris\core";

// ===== 通过态（正面对照：判据不能「恒判放宽」）=====

/// spec §3.4 要求放行的目标态：owner=SYSTEM + `SYSTEM:(F)` + `Administrators:(F)` + `Users:(RX)`。
///
/// 这条是所有「某形态被判放宽」断言的正面对照 —— 没有它，把判据改成恒真也能让那些全绿。
#[test]
fn the_locked_down_target_state_is_not_weakened() {
    assert_eq!(judge_object(PATH_DIR, &locked_down_fixture()), vec![]);
}

/// `Users:(RX)` 不算写类权 —— 掩码层面直接钉住（`(F)` 整值并进掩码就会让这条红）。
#[test]
fn read_execute_is_not_write_class_but_full_control_is() {
    assert_eq!(MASK_READ_EXECUTE & WRITE_ACCESS_MASK, 0, "(RX) 被判成写类");
    assert_ne!(MASK_FULL & WRITE_ACCESS_MASK, 0, "(F) 没被判成写类");
    assert_ne!(MASK_MODIFY & WRITE_ACCESS_MASK, 0, "(M) 没被判成写类");
}

// ===== owner 判定 =====

#[test]
fn owner_outside_the_whitelist_is_weakened() {
    let mut sec = locked_down_fixture();
    sec.owner_sid = Some(SID_A_USER.to_owned());
    assert_eq!(
        judge_object(PATH_DIR, &sec),
        vec![AclFinding {
            path: PATH_DIR.to_owned(),
            issue: AclIssue::OwnerNotPrivileged {
                owner: SID_A_USER.to_owned()
            },
        }]
    );
}

/// owner=Administrators 同样放行（白名单是两个，不是一个）。
#[test]
fn owner_administrators_is_accepted() {
    let mut sec = locked_down_fixture();
    sec.owner_sid = Some(SID_ADMINISTRATORS.to_owned());
    assert_eq!(judge_object(PATH_DIR, &sec), vec![]);
}

#[test]
fn missing_owner_is_weakened() {
    let mut sec = locked_down_fixture();
    sec.owner_sid = None;
    assert_eq!(
        judge_object(PATH_DIR, &sec),
        vec![AclFinding {
            path: PATH_DIR.to_owned(),
            issue: AclIssue::OwnerMissing,
        }]
    );
}

// ===== 白名单（不是黑名单）=====

/// 黑名单会漏的三个形态 + 一个具体用户，逐个都必须判放宽。
///
/// `INTERACTIVE` / `CREATOR OWNER` 不是「某个用户」，任何按「列出不许出现的用户 SID」写的
/// 黑名单都挡不住它们 —— 这条就是白名单存在的理由。
#[test]
fn any_non_privileged_write_grant_is_weakened() {
    for sid in [SID_INTERACTIVE, SID_CREATOR_OWNER, SID_A_USER, SID_USERS] {
        let mut sec = locked_down_fixture();
        sec.dacl.as_mut().unwrap().push(allow(sid, MASK_MODIFY));
        assert_eq!(
            judge_object(PATH_DIR, &sec),
            vec![AclFinding {
                path: PATH_DIR.to_owned(),
                issue: AclIssue::WritableBySid {
                    sid: sid.to_owned(),
                    mask: MASK_MODIFY,
                },
            }],
            "{sid} 被授写类权却没判放宽"
        );
    }
}

/// 写类掩码的**每一位**独立钉住（不是只认 `(F)`/`(M)` 这种复合形态）。
///
/// 这里的十个值是**独立写死的十六进制字面量**，刻意不从 [`WRITE_ACCESS_MASK`] 派生 —— 引用它就是
/// 判据被自己污染：编译期断言的取材面就是那份常量本身，两边同时删一位照样编得过、测得过。
/// 少覆盖一位的代价是真的：`FILE_DELETE_CHILD`(0x40) 此前既不在掩码里、也不在本表里，于是
/// `icacls <dir> /grant "*S-1-5-32-545:(DC)"` 之后普通用户可随时删掉 `sing-box.exe`
/// （删子项**绕过子项自己的 DACL**）⇒ `CreateProcessW` 恒失败 = 永久 DoS，而自检报「一切正常」。
const WRITE_BITS: [(u32, &str); 10] = [
    (0x0000_0002, "FILE_WRITE_DATA"),
    (0x0000_0004, "FILE_APPEND_DATA"),
    (0x0000_0010, "FILE_WRITE_EA"),
    (0x0000_0040, "FILE_DELETE_CHILD"),
    (0x0000_0100, "FILE_WRITE_ATTRIBUTES"),
    (0x0001_0000, "DELETE"),
    (0x0004_0000, "WRITE_DAC"),
    (0x0008_0000, "WRITE_OWNER"),
    (0x1000_0000, "GENERIC_ALL"),
    (0x4000_0000, "GENERIC_WRITE"),
];

/// 单个写位也算 —— 十位逐个跑，每位一条独立字面量。
#[test]
fn a_single_write_bit_is_enough_to_be_weakened() {
    // WRITE_DAC 单独授出 = 「你可以把 DACL 改成任何样子」，比直接授写数据更彻底；
    // FILE_DELETE_CHILD 单独授出 = 「你可以删掉核」，效果是永久 DoS。
    for (mask, name) in WRITE_BITS {
        let mut sec = locked_down_fixture();
        sec.dacl.as_mut().unwrap().push(allow(SID_A_USER, mask));
        assert_eq!(
            judge_object(PATH_DIR, &sec),
            vec![AclFinding {
                path: PATH_DIR.to_owned(),
                issue: AclIssue::WritableBySid {
                    sid: SID_A_USER.to_owned(),
                    mask,
                },
            }],
            "{name}(0x{mask:08x}) 未判放宽"
        );
    }
}

/// 上一条只能抓「掩码少了一位」；这条抓反向 —— 掩码**多**了一位而本表没跟上。
///
/// 两条断言的取材面互相独立（一边是十条字面量的并，一边是常量本身），故不是重言。
#[test]
fn the_write_mask_is_exactly_those_ten_bits() {
    let union = WRITE_BITS.iter().fold(0u32, |acc, (bit, _)| acc | bit);
    assert_eq!(
        union, WRITE_ACCESS_MASK,
        "写类掩码与逐位表分叉：某一位改了却只改了一边"
    );
    // 正面断言：十位互不相同（表里写重了会让 union 看起来仍然对，而覆盖面少一位）。
    for (i, (bit, name)) in WRITE_BITS.iter().enumerate() {
        assert_eq!(bit.count_ones(), 1, "{name} 不是单个位");
        assert!(
            !WRITE_BITS[..i].iter().any(|(b, _)| b == bit),
            "{name} 在表里重复"
        );
    }
}

/// `FILE_DELETE_CHILD` 的真实攻击形态：只授 `(DC)` 给 Users，目录的其它权限一律不动。
///
/// 这是 `a_single_write_bit_is_enough_to_be_weakened` 的具名场景对照 —— 那条是逐位普查，
/// 这条说清楚为什么 0x40 不是「目录遍历」那类无害位。
#[test]
fn delete_child_alone_is_a_write_class_grant() {
    let mut sec = locked_down_fixture();
    sec.dacl
        .as_mut()
        .unwrap()
        .push(allow(SID_USERS, 0x0000_0040));
    assert_eq!(
        judge_object(PATH_DIR, &sec),
        vec![AclFinding {
            path: PATH_DIR.to_owned(),
            issue: AclIssue::WritableBySid {
                sid: SID_USERS.to_owned(),
                mask: 0x0000_0040,
            },
        }],
        "Users:(DC) 能删掉核却没判放宽"
    );
    // 反向对照：同样孤零零一位的 `FILE_READ_EA`(0x8) 不是写类 —— 上面那条不是「多加一位就报」。
    let mut benign = locked_down_fixture();
    benign
        .dacl
        .as_mut()
        .unwrap()
        .push(allow(SID_USERS, 0x0000_0008));
    assert_eq!(judge_object(PATH_DIR, &benign), vec![]);
}

// ===== 陷阱 1：NULL DACL ≠ 空 DACL =====

/// NULL DACL 的语义是「所有人完全访问」⇒ 必判放宽，且不得被正面断言「通过」掩过去。
#[test]
fn null_dacl_is_weakened() {
    let sec = ObjectSecurity {
        owner_sid: Some(SID_LOCAL_SYSTEM.to_owned()),
        dacl: None,
    };
    assert_eq!(
        judge_object(PATH_DIR, &sec),
        vec![AclFinding {
            path: PATH_DIR.to_owned(),
            issue: AclIssue::NullDacl,
        }]
    );
}

/// 非 NULL 的**空** DACL 语义是「拒绝所有人」⇒ 由正面断言判异常，且 findings 与 NULL DACL
/// **不同形**（两者混成一个分支，拿到错误的人就分不清是「谁都能写」还是「谁都写不了」）。
#[test]
fn an_empty_dacl_fails_the_positive_assertion_and_is_not_reported_as_null() {
    let sec = ObjectSecurity {
        owner_sid: Some(SID_LOCAL_SYSTEM.to_owned()),
        dacl: Some(vec![]),
    };
    let found = judge_object(PATH_DIR, &sec);
    assert_eq!(
        found,
        vec![
            AclFinding {
                path: PATH_DIR.to_owned(),
                issue: AclIssue::MissingWriteFor {
                    sid: SID_LOCAL_SYSTEM.to_owned()
                },
            },
            AclFinding {
                path: PATH_DIR.to_owned(),
                issue: AclIssue::MissingWriteFor {
                    sid: SID_ADMINISTRATORS.to_owned()
                },
            },
        ]
    );
    assert!(
        !found.iter().any(|f| f.issue == AclIssue::NullDacl),
        "空 DACL 被报成 NULL DACL"
    );
}

// ===== 陷阱 2：INHERIT_ONLY_ACE 对对象自身不生效 =====

/// 只往下继承的写授权**不是**对本目录的授权 —— 算进去就是假红（正常安装脚本会产出这种 ACE）。
#[test]
fn an_inherit_only_write_grant_is_not_a_grant_on_the_object() {
    let mut sec = locked_down_fixture();
    let mut ace = allow(SID_A_USER, MASK_MODIFY);
    ace.flags = ACE_FLAG_INHERIT_ONLY | 0x3; // (OI)(CI)(IO)
    sec.dacl.as_mut().unwrap().push(ace);
    assert_eq!(judge_object(PATH_DIR, &sec), vec![]);
}

/// 反向对照：同一条 ACE 去掉 `INHERIT_ONLY` 立刻判放宽 —— 上一条的「放行」不是因为判据瞎了。
#[test]
fn the_same_ace_without_inherit_only_is_weakened() {
    let mut sec = locked_down_fixture();
    sec.dacl
        .as_mut()
        .unwrap()
        .push(allow(SID_A_USER, MASK_MODIFY));
    assert_eq!(judge_object(PATH_DIR, &sec).len(), 1);
}

/// INHERIT_ONLY 的 ACE 也不能充当正面断言的证据：SYSTEM 只有一条 `(IO)` 授权 ⇒ 它对本对象
/// 其实没有写权，必须判异常。
#[test]
fn an_inherit_only_grant_does_not_satisfy_the_positive_assertion() {
    let mut system_io = allow(SID_LOCAL_SYSTEM, MASK_FULL);
    system_io.flags = ACE_FLAG_INHERIT_ONLY | 0x3;
    let sec = ObjectSecurity {
        owner_sid: Some(SID_LOCAL_SYSTEM.to_owned()),
        dacl: Some(vec![system_io, allow(SID_ADMINISTRATORS, MASK_FULL)]),
    };
    assert_eq!(
        judge_object(PATH_DIR, &sec),
        vec![AclFinding {
            path: PATH_DIR.to_owned(),
            issue: AclIssue::MissingWriteFor {
                sid: SID_LOCAL_SYSTEM.to_owned()
            },
        }]
    );
}

// ===== 陷阱 3：DENY 不是授予，但针对特权侧的 DENY 是异常 =====

/// 管理员显式拒掉某个用户的写 —— 那是收紧，不是放宽。判成违规就是假红。
#[test]
fn a_deny_ace_for_a_user_is_not_a_write_grant() {
    let mut sec = locked_down_fixture();
    sec.dacl.as_mut().unwrap().push(deny(SID_A_USER, MASK_FULL));
    assert_eq!(judge_object(PATH_DIR, &sec), vec![]);
}

/// 但针对 SYSTEM / Administrators 写类权的 DENY 会让 helper 自己装不进核 ⇒ 判异常，不许装瞎。
#[test]
fn a_deny_ace_against_a_privileged_sid_is_an_anomaly() {
    for sid in PRIVILEGED_SIDS {
        let mut sec = locked_down_fixture();
        sec.dacl.as_mut().unwrap().insert(0, deny(sid, MASK_MODIFY));
        assert_eq!(
            judge_object(PATH_DIR, &sec),
            vec![AclFinding {
                path: PATH_DIR.to_owned(),
                issue: AclIssue::WriteDeniedForPrivileged {
                    sid: sid.to_owned(),
                    mask: MASK_MODIFY,
                },
            }],
            "{sid} 的写被 DENY 却没判异常"
        );
    }
}

/// 只读的 DENY（拒 `(RX)`）不碰写类权 ⇒ 不进本判据（判据只管「能不能改核」）。
#[test]
fn a_read_only_deny_is_out_of_scope() {
    let mut sec = locked_down_fixture();
    sec.dacl
        .as_mut()
        .unwrap()
        .push(deny(SID_USERS, MASK_READ_EXECUTE));
    assert_eq!(judge_object(PATH_DIR, &sec), vec![]);
}

// ===== 正面断言 =====

#[test]
fn a_missing_privileged_write_grant_is_weakened() {
    for drop_sid in PRIVILEGED_SIDS {
        let mut sec = locked_down_fixture();
        sec.dacl.as_mut().unwrap().retain(|a| a.sid != drop_sid);
        assert_eq!(
            judge_object(PATH_DIR, &sec),
            vec![AclFinding {
                path: PATH_DIR.to_owned(),
                issue: AclIssue::MissingWriteFor {
                    sid: drop_sid.to_owned()
                },
            }],
            "{drop_sid} 没有写类权却判通过"
        );
    }
}

/// 特权 SID 只被授了 `(RX)` 也算缺写类权（有 ACE ≠ 有写权）。
#[test]
fn a_privileged_sid_with_only_read_execute_is_missing_write() {
    let sec = ObjectSecurity {
        owner_sid: Some(SID_LOCAL_SYSTEM.to_owned()),
        dacl: Some(vec![
            allow(SID_LOCAL_SYSTEM, MASK_READ_EXECUTE),
            allow(SID_ADMINISTRATORS, MASK_FULL),
        ]),
    };
    assert_eq!(
        judge_object(PATH_DIR, &sec),
        vec![AclFinding {
            path: PATH_DIR.to_owned(),
            issue: AclIssue::MissingWriteFor {
                sid: SID_LOCAL_SYSTEM.to_owned()
            },
        }]
    );
}

// ===== 布局判读不了的 ACE（失败向关）=====

#[test]
fn an_unparsed_ace_is_weakened() {
    let mut sec = locked_down_fixture();
    sec.dacl.as_mut().unwrap().push(Ace {
        sid: String::new(),
        mask: 0,
        kind: AceKind::Unparsed(9), // ACCESS_ALLOWED_CALLBACK_ACE_TYPE
        flags: 0x3,
    });
    assert_eq!(
        judge_object(PATH_DIR, &sec),
        vec![AclFinding {
            path: PATH_DIR.to_owned(),
            issue: AclIssue::UnparsedAce { ace_type: 9 },
        }]
    );
}

/// 搬运腿读 **ACL/ACE 结构本身**失败时填的哨兵同样失败向关（S1）。
///
/// 这一格此前是折成 `Err` 的（`GetAclInformation` / `GetAce` 失败 → 「这个对象读不到」→ warn
/// 继续），而 `GetNamedSecurityInfoW` 已经成功返回、对象是**读到了**的 —— Q9 的「读不到 ≠ 被放宽」
/// 前提在那里不成立，于是一条读不出来的 ACE 能把同对象上另一条真放宽的 ACE 一起藏掉。
#[test]
fn the_unreadable_ace_sentinel_is_fail_closed() {
    let mut sec = locked_down_fixture();
    // 用**搬运腿真正会构造的那个值**（`Ace::unreadable()`），不是在测试里另抄一份 —— 抄一份的话
    // `flags` 被改成 `INHERIT_ONLY` 时生产侧静默失效而本条照绿。
    sec.dacl.as_mut().unwrap().push(Ace::unreadable());
    assert_eq!(
        judge_object(PATH_DIR, &sec),
        vec![AclFinding {
            path: PATH_DIR.to_owned(),
            issue: AclIssue::UnparsedAce {
                ace_type: ACE_TYPE_UNREADABLE
            },
        }]
    );
    // 正面断言哨兵对**对象自身生效** —— 这是它能被判到的充要条件（带 `INHERIT_ONLY` 就会被跳过，
    // 「读不出来的 ACE」静默变成「没有这条 ACE」）。
    assert!(Ace::unreadable().is_effective_on_object());
}

/// 哨兵值不得与任何真实 AceType 撞号 —— 撞了就没法在真机上分辨「没见过的 ACE 类型」与
/// 「`GetAce` 自己失败」，而两者的下一步排查方向完全不同。
#[test]
fn the_unreadable_sentinel_does_not_collide_with_a_real_ace_type() {
    // Win32 定义的 AceType 只用到 0x00..=0x15；allow/deny-plain 是 0 / 1。
    // 编译期算得出来 ⇒ 写成 const 断言（clippy::assertions_on_constants）。
    const { assert!(ACE_TYPE_UNREADABLE > 0x15) };
    // 且它落在 AclIssue 的 detail 里说得出来（真机上只有这一行）。
    let detail = AclIssue::UnparsedAce {
        ace_type: ACE_TYPE_UNREADABLE,
    }
    .to_string();
    assert!(detail.contains("255"), "{detail}");
}

/// 但 `INHERIT_ONLY` 的不可判读 ACE 对本对象不生效 ⇒ 不判（陷阱 2 对所有 ACE 类型一致）。
#[test]
fn an_inherit_only_unparsed_ace_is_ignored() {
    let mut sec = locked_down_fixture();
    sec.dacl.as_mut().unwrap().push(Ace {
        sid: String::new(),
        mask: 0,
        kind: AceKind::Unparsed(9),
        flags: ACE_FLAG_INHERIT_ONLY,
    });
    assert_eq!(judge_object(PATH_DIR, &sec), vec![]);
}

// ===== 定位信息 =====

/// 发现必须说得出「哪个路径、哪条 ACE/owner」—— 真机上只有一行 `ERR coredir-acl-weakened`
/// 时，这串 detail 是唯一的定位依据。
#[test]
fn findings_render_path_and_trigger() {
    let file = r"C:\ProgramData\Polaris\core\sing-box.exe";
    let mut sec = locked_down_fixture();
    sec.dacl
        .as_mut()
        .unwrap()
        .push(allow(SID_A_USER, MASK_MODIFY));
    let outcome = judge_core_objects(&[(file.to_owned(), Ok(sec))]);
    let detail = outcome.findings_detail();
    assert!(detail.contains(file), "detail 没说是哪个路径：{detail}");
    assert!(
        detail.contains(SID_A_USER),
        "detail 没说是哪条 ACE：{detail}"
    );
    assert!(detail.contains("0x001301bf"), "detail 没带掩码：{detail}");
    assert!(
        !detail.contains('\n'),
        "detail 含换行会破坏行协议帧：{detail}"
    );
}

// ===== Q9：放宽 vs 读不到，两个篮子 =====

/// 读不到的对象只进 `unreadable`，**不进** findings —— 合并就是「一次 FFI 失败 = 连不上网」。
#[test]
fn an_unreadable_object_is_not_reported_as_weakened() {
    let outcome = judge_core_objects(&[
        (PATH_DIR.to_owned(), Ok(locked_down_fixture())),
        (
            r"C:\ProgramData\Polaris\core\libcronet.dll".to_owned(),
            Err("GetNamedSecurityInfoW failed: 2".to_owned()),
        ),
    ]);
    assert!(!outcome.is_weakened(), "读不到被判成了放宽");
    assert_eq!(outcome.unreadable.len(), 1);
    assert!(outcome.unreadable_detail().contains("libcronet.dll"));
}

/// 目录锁得住、但**文件**被单独放宽，照样要判出来（覆盖面 = 目录 + 每个白名单文件）。
#[test]
fn a_weakened_file_is_caught_even_when_the_directory_is_locked_down() {
    let file = r"C:\ProgramData\Polaris\core\sing-box.exe";
    let mut weak = locked_down_fixture();
    weak.dacl
        .as_mut()
        .unwrap()
        .push(allow(SID_USERS, MASK_FULL));
    let outcome = judge_core_objects(&[
        (PATH_DIR.to_owned(), Ok(locked_down_fixture())),
        (file.to_owned(), Ok(weak)),
    ]);
    assert!(outcome.is_weakened());
    assert_eq!(outcome.findings.len(), 1);
    assert_eq!(outcome.findings[0].path, file);
}

#[test]
fn an_all_locked_down_group_passes() {
    let objects: Vec<(String, Result<ObjectSecurity, String>)> =
        core_acl_targets(r"C:\ProgramData\Polaris\core\sing-box.exe")
            .unwrap()
            .into_iter()
            .map(|p| (p, Ok(locked_down_fixture())))
            .collect();
    let outcome = judge_core_objects(&objects);
    assert!(!outcome.is_weakened());
    assert!(outcome.unreadable.is_empty());
}

// ===== 取材面 =====

/// Windows 路径在 **Linux** 上也必须切得对 —— `Path::parent()` 在这里会返回空串，
/// 于是本机全部 ACL 测试取材面为空、门恒绿。
#[test]
fn targets_cover_the_exec_dir_the_binary_and_the_cronet_dll() {
    let targets = core_acl_targets(r"C:\ProgramData\Polaris\core\sing-box.exe").unwrap();
    assert_eq!(
        targets,
        vec![
            r"C:\ProgramData\Polaris\core".to_owned(),
            r"C:\ProgramData\Polaris\core\sing-box.exe".to_owned(),
            r"C:\ProgramData\Polaris\core\libcronet.dll".to_owned(),
        ]
    );
}

/// 自检的目录是**实际执行面**（`--singbox` 的父目录），不是派生的 `<support>\core`。
#[test]
fn targets_follow_the_actual_exec_path_not_the_derived_core_dir() {
    let targets = core_acl_targets(r"C:\Users\bob\AppData\Local\Polaris\sing-box.exe").unwrap();
    assert_eq!(targets[0], r"C:\Users\bob\AppData\Local\Polaris");
}

#[test]
fn targets_are_undeterminable_for_a_bare_filename_or_empty_path() {
    assert_eq!(core_acl_targets("sing-box.exe"), None);
    assert_eq!(core_acl_targets(""), None);
}

/// 盘符根形态保留尾分隔符（`C:` 是「C 盘的当前目录」，不是根）。
#[test]
fn targets_keep_the_drive_root_separator() {
    let targets = core_acl_targets(r"C:\sing-box.exe").unwrap();
    assert_eq!(targets[0], r"C:\");
    assert_eq!(targets[2], r"C:\libcronet.dll");
}

// ===== 取材面：目录的实际条目（M2）=====

/// 硬编码的两个白名单名挡不住「攻击者预创建的第三个文件」—— 那个文件必须由**枚举**带进来。
///
/// 攻击路径全程 Medium IL：预创建 `core` 目录，放一个文件，给它设 `SE_DACL_PROTECTED`（去继承）
/// 的 DACL 含 `Users:(F)`；安装脚本的 `/setowner /T` 收得走 owner，但 `/inheritance:r` + `/grant:r`
/// 的传播**按定义跳过 protected DACL 的子项** ⇒ 那条 `Users:(F)` 原样留着。
#[test]
fn dir_entries_extend_the_target_face_beyond_the_fallback_names() {
    let base = core_acl_targets(r"C:\ProgramData\Polaris\core\sing-box.exe").unwrap();
    let got = extend_targets_with_dir_entries(
        &base,
        r"C:\ProgramData\Polaris\core",
        &["evil.dll".to_owned()],
    );
    assert_eq!(
        got,
        vec![
            r"C:\ProgramData\Polaris\core".to_owned(),
            r"C:\ProgramData\Polaris\core\sing-box.exe".to_owned(),
            r"C:\ProgramData\Polaris\core\libcronet.dll".to_owned(),
            r"C:\ProgramData\Polaris\core\evil.dll".to_owned(),
        ],
        "目录里实际躺着的第四个对象没进取材面"
    );
}

/// 枚举必然重报白名单那两个名字 —— 不去重则同一对象被判两遍、findings 出现重复条目。
///
/// 去重走 Windows 路径语义（大小写不敏感 + `/`≡`\`），不是裸字符串相等：真机 `read_dir` 报的是
/// 磁盘上的大小写，与我们拼的字面量不必逐字相同。
#[test]
fn dir_entries_are_deduplicated_against_the_fallback_names() {
    let base = core_acl_targets(r"C:\ProgramData\Polaris\core\sing-box.exe").unwrap();
    let got = extend_targets_with_dir_entries(
        &base,
        r"C:\ProgramData\Polaris\core",
        // 真机大小写与我们拼的不同，且顺序与兜底表不同。
        &["LIBCRONET.DLL".to_owned(), "Sing-Box.exe".to_owned()],
    );
    assert_eq!(got, base, "枚举出的白名单名被重复加了一遍");
}

/// 空目录 / 枚举到零条目 ⇒ 取材面**恰好**退化成兜底白名单（不是归零）。
///
/// 「文件不存在也要问一次」的语义在这里：枚举腿失败或目录为空时，覆盖面不能变成 0 个对象。
#[test]
fn an_empty_dir_listing_leaves_the_fallback_face_intact() {
    let base = core_acl_targets(r"C:\ProgramData\Polaris\core\sing-box.exe").unwrap();
    assert_eq!(
        extend_targets_with_dir_entries(&base, r"C:\ProgramData\Polaris\core", &[]),
        base
    );
    assert_eq!(base.len(), 3, "兜底面不是三个对象");
}

// ===== `Users:(RX)` 在位（warn 级自曝，不拒起核）=====

/// 目标态里 `Users:(RX)` 在位 ⇒ 不自曝（正面对照：谓词不是恒 false）。
#[test]
fn the_locked_down_target_state_grants_non_privileged_read_execute() {
    assert!(grants_non_privileged_read_execute(&locked_down_fixture()));
}

/// 把 `Users:(RX)` 摘掉 ⇒ 自曝，但**不进 findings**（helper 改不了 ACL，拒起核换不来任何收益）。
///
/// 这条链此前完全静默：app 是 Medium IL，读不到受保护目录里的 dest ⇒ `protected_core_path_in`
/// 算出的文件永远判「不存在」⇒ 每次起核白推 80MB，且内核自证恒告警。
#[test]
fn a_missing_users_read_execute_grant_is_reported_but_not_a_finding() {
    let mut sec = locked_down_fixture();
    sec.dacl.as_mut().unwrap().retain(|a| a.sid != SID_USERS);
    assert!(!grants_non_privileged_read_execute(&sec));
    assert_eq!(
        judge_object(PATH_DIR, &sec),
        vec![],
        "缺 Users:(RX) 被判成了放宽 ⇒ 会拒起核"
    );
}

/// `INHERIT_ONLY` 的 `Users:(RX)` 对本对象不生效 ⇒ 同样算缺席（与写类判据同一条陷阱 2）。
#[test]
fn an_inherit_only_read_execute_grant_does_not_count() {
    let mut sec = locked_down_fixture();
    for ace in sec.dacl.as_mut().unwrap() {
        if ace.sid == SID_USERS {
            ace.flags = ACE_FLAG_INHERIT_ONLY | 0x3;
        }
    }
    assert!(!grants_non_privileged_read_execute(&sec));
}

/// NULL DACL 下所有人都有读执行权 ⇒ 本条不重复报（那个形态由 [`AclIssue::NullDacl`] 管）。
#[test]
fn a_null_dacl_is_not_reported_as_missing_read_execute() {
    let sec = ObjectSecurity {
        owner_sid: Some(SID_LOCAL_SYSTEM.to_owned()),
        dacl: None,
    };
    assert!(grants_non_privileged_read_execute(&sec));
}

// ===== 真机 ACE flags 形态 =====

/// 真机上**文件**继承来的 ACE 带 `INHERITED_ACE`(0x10)，而 `OI`/`CI`(0x3) 对非容器会被剥掉
/// —— 夹具只有 `flags: 0x3` 的形态，等于从没验过生产上最常见的那一种。
///
/// 0x10 不是 `INHERIT_ONLY`(0x8) ⇒ 对对象自身生效 ⇒ 必须判通过。
#[test]
fn an_inherited_file_ace_shape_passes() {
    let mut sec = locked_down_fixture();
    for ace in sec.dacl.as_mut().unwrap() {
        ace.flags = 0x10; // INHERITED_ACE，OI/CI 已被剥掉（文件不是容器）
    }
    assert_eq!(
        judge_object(r"C:\ProgramData\Polaris\core\sing-box.exe", &sec),
        vec![],
        "真机文件上的继承态 ACE 被误判"
    );
    assert!(grants_non_privileged_read_execute(&sec));
    // 反向对照：同一批 flags 下加一条非特权写授权仍然判得出来（不是「0x10 让判据整体瞎了」）。
    let mut weak = sec.clone();
    let mut evil = allow(SID_A_USER, MASK_MODIFY);
    evil.flags = 0x10;
    weak.dacl.as_mut().unwrap().push(evil);
    assert_eq!(judge_object(PATH_DIR, &weak).len(), 1);
}

#[test]
fn same_win_path_is_case_and_separator_insensitive() {
    assert!(same_win_path(
        r"C:\ProgramData\Polaris\core",
        r"c:/programdata/polaris/core\"
    ));
    assert!(!same_win_path(
        r"C:\ProgramData\Polaris\core",
        r"C:\Users\bob\AppData\Local\Polaris"
    ));
}

#[test]
fn privileged_sid_whitelist_is_exactly_system_and_administrators() {
    assert!(is_privileged_sid(SID_LOCAL_SYSTEM));
    assert!(is_privileged_sid(SID_ADMINISTRATORS));
    for sid in [SID_USERS, SID_INTERACTIVE, SID_CREATOR_OWNER, SID_A_USER] {
        assert!(!is_privileged_sid(sid), "{sid} 不该在白名单里");
    }
}

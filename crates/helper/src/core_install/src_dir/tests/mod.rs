use super::*;

/// 拼一条 `FILE_FULL_DIR_INFO` 形状的记录：`name_len_at` 处放名字字节数，名字从 `name_at` 起。
fn record(name: &str, next: u32, name_len_at: usize, name_at: usize) -> Vec<u8> {
    let wide: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut bytes = vec![0xAAu8; name_at];
    bytes[..4].copy_from_slice(&next.to_le_bytes());
    bytes[name_len_at..name_len_at + 4].copy_from_slice(&(wide.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&wide);
    bytes
}

fn utf16(name: &str) -> Vec<u16> {
    name.encode_utf16().collect()
}

/// Windows 目录枚举缓冲区的解析（纯逻辑，本机可测；真正的系统调用只在 Windows 上编）。
#[test]
fn dir_entry_names_are_parsed_from_a_chained_buffer() {
    // 偏移取 `FILE_FULL_DIR_INFO` 的真实布局（FileNameLength@60、FileName@68）。
    let (len_at, name_at) = (60, 68);
    let mut first = record("sing-box.exe", 0, len_at, name_at);
    // 记录按 8 字节对齐排布。
    while !first.len().is_multiple_of(8) {
        first.push(0);
    }
    let next = u32::try_from(first.len()).unwrap();
    first[..4].copy_from_slice(&next.to_le_bytes());
    let mut buf = first;
    buf.extend(record("libcronet.dll", 0, len_at, name_at));
    // 缓冲区尾部是上一批留下的旧数据：链在 next == 0 处结束，不往后读。
    buf.extend(std::iter::repeat_n(0xFFu8, 64));

    assert_eq!(
        parse_dir_entry_names(&buf, len_at, name_at).unwrap(),
        vec![utf16("sing-box.exe"), utf16("libcronet.dll")]
    );
}

#[test]
fn a_malformed_dir_buffer_is_refused_not_guessed() {
    let (len_at, name_at) = (60, 68);
    // 名字长度指到缓冲区外。
    let mut too_long = record("a", 0, len_at, name_at);
    too_long[len_at..len_at + 4].copy_from_slice(&1000u32.to_le_bytes());
    assert!(parse_dir_entry_names(&too_long, len_at, name_at).is_err());
    // 名字字节数为奇数（不是完整的 UTF-16 码元）。
    let mut odd = record("ab", 0, len_at, name_at);
    odd[len_at..len_at + 4].copy_from_slice(&3u32.to_le_bytes());
    assert!(parse_dir_entry_names(&odd, len_at, name_at).is_err());
    // 下一条的偏移指到缓冲区外。
    let dangling = record("a", 4096, len_at, name_at);
    assert!(parse_dir_entry_names(&dangling, len_at, name_at).is_err());
    // 连一条记录的头都放不下。
    assert!(parse_dir_entry_names(&[0u8; 8], len_at, name_at).is_err());
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix {
    use super::super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    /// 建一个权限 0755 的目录（不随 umask：属主判据要求组与其他人不可写）。
    fn mkdir(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn me() -> u32 {
        nix::unistd::Uid::current().as_raw()
    }

    /// 本机内核有没有 `openat2`（5.6 起）。直接问内核，不经被测代码。
    #[cfg(target_os = "linux")]
    fn openat2_available() -> bool {
        use nix::fcntl::{openat2, OpenHow, AT_FDCWD};
        !matches!(
            openat2(AT_FDCWD, "/", OpenHow::new().flags(DIR_FLAGS)),
            Err(nix::errno::Errno::ENOSYS)
        )
    }

    /// 没有 `openat2` 的内核上，`SrcDir::open` 走回退路径。那条路的既定行为是：只守最后一级
    /// 不跟随，路径中间的链接走得过去，但属主判据照过。`via_link` 是一条中间经过链接、指向
    /// 一个属本进程的 0755 目录的路径。
    #[cfg(target_os = "linux")]
    fn assert_fallback_contract(via_link: &Path) {
        println!("openat2 不可用，按回退路径断言");
        let holder = tempfile::tempdir().unwrap();
        symlink(via_link, holder.path().join("final-link")).unwrap();
        assert!(
            SrcDir::open(&holder.path().join("final-link"), Some(me())).is_err(),
            "回退路径：最后一级是链接仍须拒"
        );
        let wrong_owner = SrcDir::open(via_link, Some(me() + 1))
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            wrong_owner.as_deref(),
            Some("not owned by the caller"),
            "回退路径：属主判据仍须生效"
        );
        SrcDir::open(via_link, Some(me())).expect("回退路径：中间的链接走得过去");
    }

    /// 路径中间某一级是符号链接：`openat2` 那条路整条路径都不穿越。
    #[cfg(target_os = "linux")]
    #[test]
    fn a_symlink_in_a_parent_component_is_rejected() {
        let holder = tempfile::tempdir().unwrap();
        mkdir(&holder.path().join("real/src"));
        symlink(holder.path().join("real"), holder.path().join("link")).unwrap();
        // 正面对照：真实路径打得开。
        SrcDir::open(&holder.path().join("real/src"), Some(me())).unwrap();

        if !openat2_available() {
            assert_fallback_contract(&holder.path().join("link/src"));
            return;
        }
        assert!(SrcDir::open(&holder.path().join("link/src"), Some(me())).is_err());
    }

    /// `/proc/<pid>/root` 这类魔法链接：经它能走进别的挂载命名空间。
    #[cfg(target_os = "linux")]
    #[test]
    fn a_proc_magic_link_in_the_path_is_rejected() {
        let src = tempfile::tempdir().unwrap();
        mkdir(src.path());
        let via_proc = format!("/proc/self/root{}", src.path().display());
        // 前置条件：这条路径本身是通的（普通的打开走得过去）。
        assert!(std::fs::read_dir(&via_proc).is_ok());
        SrcDir::open(src.path(), Some(me())).unwrap();

        if !openat2_available() {
            assert_fallback_contract(Path::new(&via_proc));
            return;
        }
        assert!(SrcDir::open(Path::new(&via_proc), Some(me())).is_err());
    }

    /// 内核没有 `openat2` 时的回退：只守最后一级不跟随，但属主判据照过 —— 回退不放宽它。
    #[test]
    fn the_fallback_open_still_refuses_a_final_symlink_and_checks_the_owner() {
        let holder = tempfile::tempdir().unwrap();
        mkdir(&holder.path().join("real/src"));
        symlink(
            holder.path().join("real/src"),
            holder.path().join("final-link"),
        )
        .unwrap();
        symlink(holder.path().join("real"), holder.path().join("link")).unwrap();

        // 最后一级是链接：打不开。
        assert!(open_dir_last_component_nofollow(&holder.path().join("final-link")).is_err());

        // 中间一级是链接：回退路径打得开（这是它比 `openat2` 弱的地方）……
        let fd = open_dir_last_component_nofollow(&holder.path().join("link/src")).unwrap();
        // ……但属主不符照样拒。
        let wrong_owner = SrcDir::from_fd(fd, me() + 1).err().map(|e| e.to_string());
        assert_eq!(wrong_owner.as_deref(), Some("not owned by the caller"));

        let fd = open_dir_last_component_nofollow(&holder.path().join("link/src")).unwrap();
        SrcDir::from_fd(fd, me()).unwrap();
    }
}

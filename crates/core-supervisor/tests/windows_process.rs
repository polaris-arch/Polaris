//! Windows 进程接口的**真实进程**门：作业对象（直起的核随主程序结束）与孤儿清扫的进程扫描。
//!
//! 只在 Windows 上编译与运行；别的平台上这两件事各有各的机制，不归本文件。
//!
//! # 替身进程
//!
//! 全部用本 crate 的探针 bin（`src/bin/argv_probe.rs`）：`--sleep` 模式常驻 30 秒、不读标准输入、
//! 不开端口、不碰网络，带任意多余参数照样存活，且由 cargo 随测试一起构建 —— 不依赖机器上装了什么。
//! 系统自带的候选都各缺一条：`cmd /c pause` 与 `timeout` 要控制台输入，`ping` 走回环网络。
//! 探针可以被复制成别的文件名（孤儿清扫那条要它叫核的名字），它只链接系统运行库。
#![cfg(windows)]

use std::io::BufRead;
use std::os::windows::io::{AsHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use polaris_core_supervisor::job_object::{process_job, KillOnCloseJob};
use polaris_core_supervisor::{
    scan_running_cores, stale_pids, CoreProcess, ProcessOwner, SingBoxSpawner, SpawnRequest,
    StdioPolicy, TokioSpawner,
};
use windows_sys::Win32::Foundation::{FALSE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

/// 探针二进制绝对路径（cargo 在集成测试期注入）。
const PROBE: &str = env!("CARGO_BIN_EXE_argv_probe");

/// 「它该结束」的等待上界。内核连坐是即时的，给足余量只为不在慢机器上假红。
const MUST_EXIT_WITHIN: Duration = Duration::from_secs(15);
/// 「它该还活着」的观察时长：比连坐生效所需长得多，又远短于探针的 30 秒寿命。
const STILL_ALIVE_AFTER: Duration = Duration::from_secs(2);

fn sleeper() -> Child {
    Command::new(PROBE)
        .arg("--sleep")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("起探针")
}

/// 在 `limit` 内等到子进程退出则为真。
fn exits_within(child: &mut Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if child.try_wait().expect("查询探针状态").is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 一个不是本进程子进程的进程（孙进程）的句柄：先拿到句柄再动它的父进程，号码被复用也认不错。
struct Foreign(OwnedHandle);

impl Foreign {
    fn open(pid: u32) -> Self {
        // SAFETY: 只请求同步 / 受限查询 / 结束权限；失败返回空句柄，下面当场断言。
        let raw = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                FALSE,
                pid,
            )
        };
        assert!(!raw.is_null(), "打不开 pid={pid}：它在被观察之前就不在了");
        // SAFETY: `raw` 是刚交回、尚无其它所有者的有效句柄。
        Self(unsafe { OwnedHandle::from_raw_handle(raw.cast()) })
    }

    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        use std::os::windows::io::AsRawHandle;
        self.0.as_raw_handle().cast()
    }

    /// 在 `limit` 内等到它退出则为真。
    fn exits_within(&self, limit: Duration) -> bool {
        // SAFETY: 句柄由 `self` 保活。
        let waited = unsafe { WaitForSingleObject(self.raw(), limit.as_millis() as u32) };
        assert!(
            waited == WAIT_OBJECT_0 || waited == WAIT_TIMEOUT,
            "等待进程句柄失败：{waited:#x}"
        );
        waited == WAIT_OBJECT_0
    }

    fn kill(&self) {
        // SAFETY: 句柄由 `self` 保活且带结束权限；对已退出的进程失败无害。
        unsafe { TerminateProcess(self.raw(), 1) };
    }
}

/// 🔴 作业对象的全部语义：丢弃作业句柄 ⇒ 作业里的进程结束；作业外的同款进程照常活着。
///
/// 反向对照就在同一条用例里：两个探针同时起、同样的寿命，唯一的差别是有没有被纳入。
/// 丢弃之前先确认作业里的那个**还活着** —— 否则「它结束了」可能只是它自己退了。
#[test]
fn closing_the_job_ends_its_processes_and_only_those() {
    let mut inside = sleeper();
    let mut outside = sleeper();
    let job = KillOnCloseJob::new().expect("建作业");
    job.assign(inside.as_handle()).expect("纳入作业");
    assert!(job.contains(inside.as_handle()).expect("查询归属"));
    assert!(!job.contains(outside.as_handle()).expect("查询归属"));
    assert!(
        !exits_within(&mut inside, Duration::from_millis(500)),
        "纳入作业本身不该结束进程"
    );

    drop(job);

    assert!(
        exits_within(&mut inside, MUST_EXIT_WITHIN),
        "作业句柄关闭后，作业里的进程必须结束"
    );
    assert!(
        !exits_within(&mut outside, STILL_ALIVE_AFTER),
        "没纳入作业的进程不该受影响"
    );
    let _ = outside.kill();
    let _ = outside.wait();
}

/// 🔴 接线：经 `TokioSpawner` 起的进程就在本进程的作业里（三条直起腿共用这个 spawner）。
///
/// **牙**：删掉 `TokioSpawner::spawn` 里的纳入调用 ⇒ `contains` 为假，本条转红。
#[tokio::test]
async fn the_spawner_enrolls_every_core_in_the_process_job() {
    let mut spawned = TokioSpawner::new()
        .spawn(SpawnRequest::new(PROBE, "--sleep", StdioPolicy::Discard))
        .expect("spawn 探针应成功");
    let job = process_job().expect("本进程的作业应能建出");
    let raw = spawned.child.raw_handle().expect("子进程句柄");
    // SAFETY: 句柄由 `spawned.child` 持有，下面用完之前它不会被收割。
    let handle = unsafe { BorrowedHandle::borrow_raw(raw) };
    assert!(
        job.contains(handle).expect("查询归属"),
        "经 spawner 起的进程必须在本进程的作业里"
    );
    // 反向对照：同一个作业并不包含随手起的别的进程（`contains` 不是恒真）。
    let mut plain = sleeper();
    assert!(!job.contains(plain.as_handle()).expect("查询归属"));
    let _ = plain.kill();
    let _ = plain.wait();
    // 正常停核照旧走 Child，与作业无关。
    spawned.child.start_kill().expect("结束探针");
    let _ = spawned.child.wait().await;
}

/// 起一个冒充主程序的探针，读回它起的子进程的 pid。
fn parent_with_child(mode: &str) -> (Child, Foreign) {
    let mut parent = Command::new(PROBE)
        .arg(mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("起冒充主程序的探针");
    let mut line = String::new();
    std::io::BufReader::new(parent.stdout.take().expect("探针 stdout"))
        .read_line(&mut line)
        .expect("读子进程 pid");
    let pid: u32 = line
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("探针没有报出子进程 pid：{line:?}"));
    (parent, Foreign::open(pid))
}

/// 🔴 端到端：主程序被**强杀**（没有任何清理代码能跑）⇒ 它纳入本进程作业的子进程随之结束。
///
/// 探针里纳入用的是生产函数 `job_object::enroll_child`（`TokioSpawner` 起核后调的同一个），作业是
/// 进程级的那一个 —— 验的是「句柄活到进程结束、进程一消失内核就关掉它」这件事本身。「spawner 确实
/// 调了它」由上一条用例证明。
///
/// 反向对照是同一个探针少做一步：没纳入的子进程在父进程被强杀后照常活着。两条腿只差「是否纳入」，
/// 于是第一条的绿不能归因于「父进程一死子进程本来就会死」。
#[test]
fn a_killed_parent_takes_its_enrolled_child_with_it() {
    let (mut parent, child) = parent_with_child("--spawn-enrolled");
    assert!(
        !child.exits_within(Duration::from_millis(500)),
        "前提：父进程还在时子进程在跑"
    );
    parent.kill().expect("强杀父进程");
    let _ = parent.wait();
    assert!(
        child.exits_within(MUST_EXIT_WITHIN),
        "父进程被强杀后，它作业里的子进程必须结束"
    );

    let (mut parent, child) = parent_with_child("--spawn-plain");
    parent.kill().expect("强杀父进程");
    let _ = parent.wait();
    let survived = !child.exits_within(STILL_ALIVE_AFTER);
    child.kill();
    assert!(survived, "对照：没纳入作业的子进程不该随父进程结束");
}

/// 一棵带空格的临时目录树，析构时删除。
struct Tree(std::path::PathBuf);

impl Tree {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("polaris stale scan {}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("建临时目录");
        Self(root)
    }

    fn dir(&self, name: &str) -> std::path::PathBuf {
        let dir = self.0.join(name);
        std::fs::create_dir_all(&dir).expect("建子目录");
        dir
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn spawn_core_shaped(binary: &Path, config: &Path) -> polaris_core_supervisor::SpawnedChild {
    let mut request = SpawnRequest::new(binary, config, StdioPolicy::Discard);
    // 探针按这个参数常驻；对判据而言它只是 `run -c <配置>` 之后的尾随参数。
    request.extra_args = vec!["--sleep".to_owned()];
    TokioSpawner::new().spawn(request).expect("起核形状的探针")
}

/// 🔴 孤儿清扫的扫描侧：真实进程的命令行读得出来、切得对，且同一判据只选中本 app 形状的那个。
///
/// 探针被复制成核的文件名，经生产的 `TokioSpawner` 以 `run -c <配置目录>\<x>.json` 起 ——
/// 命令行是系统按真实规则拼出来的，程序路径与配置目录都带空格。
///
/// 对照有两个：配置在配置目录**子目录**里的同名进程不被选中；排除表里的 pid 不被选中。
#[tokio::test]
async fn the_scan_reads_real_command_lines_and_selects_only_our_shape() {
    const CORE: &str = "sing-box.exe";
    let tree = Tree::new();
    let binary = tree.dir("bin dir").join(CORE);
    std::fs::copy(PROBE, &binary).expect("把探针复制成核的文件名");
    let config_dir = tree.dir("conf dir");
    let ours_config = config_dir.join("singbox-runtime.json");
    let foreign_config = tree.dir("conf dir/sub").join("singbox-runtime.json");

    let mut ours = spawn_core_shaped(&binary, &ours_config);
    let mut foreign = spawn_core_shaped(&binary, &foreign_config);
    let ours_pid = ours.pid().expect("pid");
    let foreign_pid = foreign.pid().expect("pid");

    let scanned = scan_running_cores();
    let argv_of = |pid: u32| {
        scanned
            .iter()
            .find(|process| process.pid == pid)
            .map(|process| process.cmdline.clone())
    };
    let selected = stale_pids(&scanned, CORE, &config_dir, &[]);
    let excluded = stale_pids(&scanned, CORE, &config_dir, &[ours_pid]);

    // 先收尾再断言：断言失败也不留进程（它们同时在本测试进程的作业里，进程退出时也会被带走）。
    for child in [&mut ours, &mut foreign] {
        let _ = child.child.start_kill();
        let _ = child.child.wait().await;
    }

    assert!(
        scanned
            .iter()
            .any(|process| process.pid == std::process::id()),
        "扫描结果里应有本测试进程自己"
    );
    assert_eq!(
        argv_of(ours_pid),
        Some(vec![
            binary.to_string_lossy().into_owned(),
            "run".to_owned(),
            "-c".to_owned(),
            ours_config.to_string_lossy().into_owned(),
            "--sleep".to_owned(),
        ]),
        "读回的命令行必须切回起它时的 argv（路径含空格）"
    );
    assert!(argv_of(foreign_pid).is_some(), "对照进程也应被扫描到");
    assert!(selected.contains(&ours_pid), "本 app 形状的进程必须被选中");
    assert!(
        !selected.contains(&foreign_pid),
        "配置在子目录里的同名进程不归本 app"
    );
    assert!(!excluded.contains(&ours_pid), "排除表里的 pid 不得被选中");
}

/// 🔴 孤儿清扫的结束侧：扫描到的进程带着属主与扫描时留住的句柄；经它结束的只是这一个进程，
/// 不连带它的子进程，且已退出的进程不会被再「结束」一次。
///
/// 被结束的是一个带着子进程的探针（`--spawn-plain`，子进程没纳入任何由它持有的作业）：父进程
/// 结束后子进程照常活着，说明结束不带进程树 —— 旧写法 `taskkill /T` 会把子进程一起带走。
#[test]
fn a_scanned_process_is_ended_alone_through_its_held_handle() {
    let (mut parent, child) = parent_with_child("--spawn-plain");
    let scanned = scan_running_cores()
        .into_iter()
        .find(|process| process.pid == parent.id());

    let owner = scanned.as_ref().map(|process| process.owner);
    let running_before = scanned.as_ref().and_then(|p| p.hold.is_running());
    let ended = scanned.as_ref().is_some_and(|p| p.hold.terminate());
    let parent_exited = exits_within(&mut parent, MUST_EXIT_WITHIN);
    let running_after = scanned.as_ref().and_then(|p| p.hold.is_running());
    let child_survived = !child.exits_within(STILL_ALIVE_AFTER);
    let ended_again = scanned.as_ref().is_some_and(|p| p.hold.terminate());

    // 先收尾再断言。
    child.kill();
    if !parent_exited {
        let _ = parent.kill();
    }
    let _ = parent.wait();

    assert_eq!(
        owner,
        Some(ProcessOwner::SameUser),
        "自己起的进程应被扫描到，且属主是本用户"
    );
    assert_eq!(running_before, Some(true), "结束之前经句柄看它在跑");
    assert!(ended, "本用户的进程必须结束得了");
    assert!(parent_exited, "被结束的进程必须在上界内退出");
    assert_eq!(running_after, Some(false), "退出之后经句柄看它不在跑");
    assert!(child_survived, "结束不带进程树：它的子进程必须还活着");
    assert!(!ended_again, "已退出的进程不再被结束");

    // 没带句柄的候选（不是扫描出来的）：不结束任何进程，也说不出在不在跑。
    let bare = CoreProcess {
        pid: std::process::id(),
        ..CoreProcess::default()
    };
    assert!(!bare.hold.terminate());
    assert_eq!(bare.hold.is_running(), None);
}

//! 测试专用探针二进制：冒充 `sing-box check`，把 [`run_config_check`](polaris_core_supervisor::run_config_check) 的真实接线变成可断言的行为。
//! 不进任何发布产物。
//!
//! **为什么需要它**（同 `argv_probe.rs` 的理由，这里再具体一层）：`run_config_check` 那段是纯接线——
//! argv 拼装、`--disable-color` 有没有真传出去、读的是 stderr 还是 stdout、退出码怎么映射三态。
//! 这些**一条都不是纯逻辑**，只能靠真跑一个子进程来验；而拿随包 `resources/linux/sing-box` 来验不行：
//! 那个二进制**不入库**（`.gitignore` 的 `/resources/*`，由 `scripts/` 拉取），于是在没跑过 fetch-core
//! 的机器与 CI 上，依赖它的测试只能写成「文件不在就 skip」= 一条**永远不会红**的门，比没有门更坏。
//! 本 crate 自己的 bin 目标三平台恒在，门没有平台盲区、也没有「资源没拉就静默失效」的盲区。
//!
//! **模式由 argv 里的配置路径选**（`run_config_check` 恒发 `--disable-color check -c <path>`，
//! 路径文件名选择模式；hang 模式读取私有配置副本中的见证路径）：
//!
//! | 配置路径含 | 行为 | 服务于 |
//! |---|---|---|
//! | `accept`   | rc=0，无输出                          | `Accepted` 腿 |
//! | `reject`   | rc=1，stderr 吐 decode 期 FATAL 行     | `Rejected` 腿 + 「读的是 stderr」 |
//! | `stdout`   | rc=1，同样的 FATAL 行只吐 **stdout**   | 「stderr 空才回落 stdout」那条兜底 |
//! | `garbage`  | rc=1，stderr 吐一行拆不出下标的话       | `Unattributable` 腿 |
//! | `silent`   | rc=1，双流全空                         | 「非零退出但无输出」的病态腿 |
//! | `argv`     | rc=1，stderr 打回完整 argv              | `--disable-color` / `check` / `-c` 真的传出去了 |
//! | `hang`     | 睡 400ms **然后**建一个见证文件再 rc=0  | 超时腿 + 「超时后子进程真的被杀掉」 |
//! | 其余       | rc=0                                   | 缺省视同通过 |
//!
//! `hang` 的配置内容是见证文件路径，探针睡满后才写入。
//! 见证只用于辅助验证延迟写入行为；缺失不是退出证明，退出事实来自同 owned Child native wait。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let joined = args.join(" ");
    if joined.contains("hang") {
        let config = args.last().expect("config path");
        let witness = std::fs::read_to_string(config).expect("probe witness config");
        std::thread::sleep(std::time::Duration::from_millis(400));
        // 睡满才落见证。被 kill 掉的话这一行永远不执行。
        let _ = std::fs::write(witness.trim(), b"done");
        return;
    }
    // 逐字取自随包 sing-box 1.14.0-beta.7 对「未知 outbound type」坏 config 的真实 stderr
    // （带 `--disable-color` 时无 ANSI 前缀）。
    const FATAL: &str =
        "FATAL[0000] decode config at cfg.json: outbounds[3]: unknown outbound type: zzz";
    if joined.contains("accept") {
        return;
    }
    if joined.contains("reject") {
        eprintln!("{FATAL}");
    } else if joined.contains("stdout") {
        println!("{FATAL}");
    } else if joined.contains("garbage") {
        eprintln!("FATAL[0000] decode config at cfg.json: duplicate outbound/endpoint tag: dup");
    } else if joined.contains("argv") {
        eprintln!("{joined}");
    } else if !joined.contains("silent") {
        return;
    }
    std::process::exit(1);
}

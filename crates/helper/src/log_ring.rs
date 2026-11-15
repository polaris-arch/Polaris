//! helper 自身日志的有界内存环形缓冲（供 App 导出时经 `helper-logs` 只读取回）。
//!
//! 不落盘、不起线程：每条记录在调用线程内格式化后入队，总字节超限丢最旧。只收 Info 及以上。
//! 记录可带 `owner` 键（unix uid 数值或 [`CallerIdentity::key`] 字符串），读取时只返回属主等于
//! 调用方或无属主的记录；`owner` 键存在却取不出属主（值为空或类型不可转换）时记为对任何调用方都
//! 不可见的 `"!"`，不退化为全局可见。

use polaris_helper_proto::core_protocol::CallerIdentity;
use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

/// 环形缓冲总字节上限（按返回正文计，含换行）。
const CAPACITY_BYTES: usize = 64 * 1024;
/// 单条记录上限，超出截断。
const ENTRY_BYTES: usize = 512;
/// 同一属主（全局记录合计为一份）的字节配额：超额只挤掉同属主的旧记录，一个用户刷屏不能
/// 冲掉其他用户与全局记录；总量仍超 `CAPACITY_BYTES` 时丢最旧的带属主记录。
const OWNER_BYTES: usize = 16 * 1024;

struct Ring {
    entries: VecDeque<(Option<String>, String)>,
    bytes: usize,
}

static RING: Mutex<Ring> = Mutex::new(Ring {
    entries: VecDeque::new(),
    bytes: 0,
});

struct RingLogger;

impl log::Log for RingLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let owner = record
            .key_values()
            .get(log::kv::Key::from_str("owner"))
            .map(|value| match value.to_u64() {
                Some(uid) => u32::try_from(uid)
                    .map_or_else(|_| "!".to_owned(), |uid| CallerIdentity::Unix(uid).key()),
                None => value.to_borrowed_str().unwrap_or("!").to_owned(),
            });
        let mut line = format!(
            "{} {} {}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis()),
            record.level(),
            record.args()
        );
        if line.len() > ENTRY_BYTES {
            line.truncate(line.floor_char_boundary(ENTRY_BYTES));
            line.shrink_to_fit();
        }
        let mut ring = RING.lock().unwrap_or_else(PoisonError::into_inner);
        ring.bytes += line.len() + 1;
        let mut used = line.len()
            + 1
            + ring
                .entries
                .iter()
                .filter(|(o, _)| *o == owner)
                .map(|(_, l)| l.len() + 1)
                .sum::<usize>();
        while used > OWNER_BYTES || ring.bytes > CAPACITY_BYTES {
            let same = used > OWNER_BYTES;
            let Some((old_owner, oldest)) = ring
                .entries
                .iter()
                .position(|(o, _)| if same { *o == owner } else { o.is_some() })
                .and_then(|at| ring.entries.remove(at))
            else {
                break;
            };
            ring.bytes -= oldest.len() + 1;
            if old_owner == owner {
                used -= oldest.len() + 1;
            }
        }
        ring.entries.push_back((owner, line));
    }

    fn flush(&self) {}
}

/// daemon 启动时安装（进程内只装一次；已有 logger 时不覆盖）。
pub fn init() {
    if log::set_logger(&RingLogger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
        log::info!(
            "helper started build={} proto={}",
            polaris_helper_proto::build_identity::current(),
            polaris_helper_proto::proto_version::CURRENT
        );
    }
}

/// 按调用方过滤后的日志正文（每条一行，总量不超过 `CAPACITY_BYTES`）。
#[must_use]
pub fn read(caller: Option<&str>) -> String {
    let ring = RING.lock().unwrap_or_else(PoisonError::into_inner);
    let mut out = String::new();
    for (owner, line) in &ring.entries {
        if owner.is_none() || owner.as_deref() == caller {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

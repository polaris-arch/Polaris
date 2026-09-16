// vendored proto ⇄ 随包内核 wire 契约对拍（纯 std，零依赖）。
//
// # 这个文件为什么存在
//
// `tests/mock_server.rs` 用**同一份 vendored 类型**同时当 client 和 server —— 它验证的是「本仓自己
// 跟自己一致」，对「本仓与真核一致吗」这个问题结构上无法给出任何信息。2026-08-05 那次故障正好落在
// 它的盲区里：上游在 `TailscaleEndpointStatus.stateText` 插了一个字段把后面全顶掉一位，mock 测试
// 全绿，真机上整条 Tailscale STATUS 流静默死掉（详见 `proto/started_service.proto` 的对应段落）。
//
// 本模块补的就是那一格：**从随包内核二进制里把 protoc-gen-go 嵌入的 `FileDescriptorProto` 抠出来，
// 与 vendored `.proto` 文本逐字段对账**。真值取自将要被执行的那个二进制本身，不是文档、不是记忆。
//
// # 为什么手写 protobuf 解析而不引 crate
//
// 只需要 descriptor 里的三层：文件 → message → (字段名, 字段号)。全部是 varint + length-delimited
// 两种 wire type，~80 行 std 足够；为一道构建期门引 `prost-types` / `protobuf-parse` 属反向依赖
// （门本身就是用来发现 proto 层出问题的，不该再依赖一层 proto 库）。
//
// # 被谁 include
//
// `build.rs`（release 构型硬门，拒绝出包）与 `tests/bundled_core_wire.rs`（开发机 / 有核时）各
// `include!` 一次。不做成 crate 内的 `mod`：build script 不能依赖它正在构建的那个 crate。
#[allow(dead_code)]
mod proto_wire_check {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// protoc-gen-go 把 `FileDescriptorProto` 原样嵌进 .rodata；它以 field 1(name) 起头，
    /// 故 `0x0A <len> "daemon/started_service.proto"` 这串字节就是它的锚点。
    const PROTO_FILE_NAME: &[u8] = b"daemon/started_service.proto";

    /// 仓库根（本 crate 在 `crates/singbox-grpc/`，故上跳两级）。
    pub fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
    }

    // ── 随包核平台枚举：唯一真值源 ────────────────────────────────────────────
    //
    // 此前这里写着一份四条字面路径的表。它当时**不缺平台**，要治的是漂移：将来加一个平台
    // （例如 `linux-arm64`）时，改的是 manifest 与 `scripts/fetch-core.mjs`，而这份表不会跟 ——
    // 于是本门静默少看一个平台，且那一格不会红，没有任何信号。
    //
    // 跨 crate 共享不了 `crates/config-engine/tests/support/core_locator.rs` 的 `CORE_MATRIX`
    // （`#[path]` 跨 crate include = 把锚点锚在别人的目录结构上），故两侧都改为从**同一份 manifest**
    // 派生：平台**枚举**取自 manifest 的键集合，平台**路径形态**是一条规则（见
    // [`core_relative_path`]），不是第二份名单。

    /// 随包核平台枚举的权威真值源（相对仓库根）。
    ///
    /// `coreArchiveSha256` 的**键集合**就是「随包核有哪几个平台」：`scripts/fetch-core.mjs`
    /// 按它逐平台拉核，且缺 pin 即拒绝下载 —— 一个平台要随包，它的键必然先在这里。
    const CORE_MANIFEST_REL: &str = "src-tauri/core-manifest.json";

    /// manifest 里持有平台枚举的字段名。
    const CORE_PLATFORM_FIELD: &str = "coreArchiveSha256";

    /// 随包核平台枚举文件的绝对路径。
    pub fn core_manifest_path() -> PathBuf {
        repo_root().join(CORE_MANIFEST_REL)
    }

    /// 打包腿的硬化开关（与 `config-engine/tests/support/core_locator.rs` 的同名开关同义）：
    /// 置 1 时「manifest 声明了、盘上没有」即红；开发机不置，只报告 ——
    /// `node scripts/fetch-core.mjs --platform=linux` 只拉一份是常态。
    pub fn kernel_gate_required() -> bool {
        std::env::var("POLARIS_REQUIRE_KERNEL_GATE").is_ok_and(|v| v == "1")
    }

    /// 随包核平台枚举（manifest `coreArchiveSha256` 的键，按文件里的出现顺序）。
    ///
    /// 🔴 **读不到 / 解不出 / 一个键都没有一律 panic**，绝不返回空表：下面的普查按
    /// 「存在即检」过滤，空枚举会退化成「没有平台要查」⇒ 整道门变成恒绿的零信息量摆设。
    /// 这一档失败响亮，而退化成空表是静默的 —— 两者的代价差着一整条故障链。
    pub fn core_platforms() -> Vec<String> {
        let path = core_manifest_path();
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "读不到随包核平台枚举 {}：{e}\n\
                 该文件是「随包核有哪几个平台」的权威真值源，读不到就确定不了本门的覆盖轴 ——\
                 故直接失败，不降级成「零平台」。",
                path.display()
            )
        });
        object_keys(&src, CORE_PLATFORM_FIELD).unwrap_or_else(|e| {
            panic!(
                "{} 的 `{CORE_PLATFORM_FIELD}` 解析失败：{e}\n\
                 本门的覆盖轴就是这个字段的键集合，解不出即无法确定要看哪几个平台。",
                path.display()
            )
        })
    }

    /// 由平台 key 推导随包核在仓内的相对路径。**本 crate 唯一一处路径形态实现**。
    ///
    /// 规则（与 `scripts/fetch-core.mjs` 的 `TARGETS` 同一套）：目录名就是 key 本身，
    /// 二进制名只有 Windows 那一族带 `.exe`。写成规则而不是名单，是因为规则在加平台时自动跟上，
    /// 名单不会 —— 而「名单没跟上」的表现正是本次要治的那种静默缩水。
    pub fn core_relative_path(key: &str) -> String {
        let bin = if key == "win" || key.starts_with("win-") {
            "sing-box.exe"
        } else {
            "sing-box"
        };
        format!("resources/{key}/{bin}")
    }

    /// manifest 声明的每个平台及其随包核绝对路径（不管盘上在不在）。
    pub fn core_paths() -> Vec<(String, PathBuf)> {
        let root = repo_root();
        core_platforms()
            .into_iter()
            .map(|key| {
                let path = root.join(core_relative_path(&key));
                (key, path)
            })
            .collect()
    }

    /// 盘上随包核的一次普查。三条分别对应三种「覆盖轴出问题」的形态。
    pub struct CoreSurvey {
        /// manifest 声明、盘上也有：本门真正能对拍的那几份。
        pub present: Vec<(String, PathBuf)>,
        /// manifest 声明、盘上没有：开发机只拉一份是常态，打包腿上则是 fetch 失败。
        pub missing: Vec<String>,
        /// 盘上有 sing-box 二进制、manifest 里却没有对应平台 —— **孤儿核**。
        /// 它会被 tauri 的 resources 照样打进包，却不在任何一道逐平台门的覆盖轴上。
        pub orphans: Vec<String>,
    }

    /// 普查盘上的随包核。覆盖轴来自 manifest，孤儿来自 `resources/` 的实际目录。
    pub fn survey_bundled_cores() -> CoreSurvey {
        let declared = core_paths();
        let mut present = Vec::new();
        let mut missing = Vec::new();
        for (key, path) in &declared {
            if path.is_file() {
                present.push((key.clone(), path.clone()));
            } else {
                missing.push(key.clone());
            }
        }
        assert_eq!(
            present.len() + missing.len(),
            declared.len(),
            "普查漏掉了平台 —— 每个声明过的平台必须落进 present 或 missing 其中一格"
        );

        // 孤儿：`resources/<dir>/` 里有 sing-box 二进制，但 <dir> 不在 manifest 的键集合里。
        // `resources/` 下还有 dashboard / data 等非平台目录，故判据是「这个目录里有没有核」
        // 而不是「这个目录叫什么」。读不到 `resources/` 时（裸 checkout）没有孤儿可言。
        let declared_keys: Vec<&str> = declared.iter().map(|(key, _)| key.as_str()).collect();
        let mut orphans = Vec::new();
        if let Ok(entries) = std::fs::read_dir(repo_root().join("resources")) {
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if declared_keys.contains(&name.as_str()) {
                    continue;
                }
                let dir = entry.path();
                if dir.join("sing-box").is_file() || dir.join("sing-box.exe").is_file() {
                    orphans.push(name);
                }
            }
        }
        orphans.sort();

        CoreSurvey {
            present,
            missing,
            orphans,
        }
    }

    // ── 最小 JSON 键读取 ──────────────────────────────────────────────────────

    /// 取 JSON 里某个对象字段的**直接键名**（按出现顺序）。
    ///
    /// 手写而不引 JSON crate，理由同本文件开头那段 protobuf 解析：本模块被 `build.rs` /
    /// `src/lib.rs` / 集成测试三处 `include!`，引一个 crate 要同时进 `[build-dependencies]`、
    /// `[dependencies]`、`[dev-dependencies]` 三处 —— 为一道门把 JSON 解析塞进**运行期**依赖图
    /// 不划算。manifest 结构固定、字段名固定，这里只需要「某个对象的键名」这一格。
    ///
    /// 🔴 **任何看不懂的形态一律 `Err`，绝不返回空表** —— 空表会被消费点读成「没有平台要查」，
    /// 那是静默失效；`Err` 在消费点变成 panic，是响亮失效。
    pub fn object_keys(src: &str, field: &str) -> Result<Vec<String>, String> {
        let needle = format!("\"{field}\"");
        let at = src
            .find(&needle)
            .ok_or_else(|| format!("找不到字段 `{field}`"))?;
        let after = src[at + needle.len()..].trim_start();
        let body = after
            .strip_prefix(':')
            .ok_or_else(|| format!("`{field}` 后面不是 `:`"))?
            .trim_start()
            .strip_prefix('{')
            .ok_or_else(|| format!("`{field}` 的值不是对象"))?;

        let bytes = body.as_bytes();
        let (mut i, mut depth) = (0usize, 1usize);
        let mut pending: Option<String> = None;
        let mut keys: Vec<String> = Vec::new();
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    let (text, next) = json_string(body, i)?;
                    pending = Some(text);
                    i = next;
                }
                // 只有本层（depth == 1）的 `"…":` 才是我们要的键；嵌套对象里的键不算。
                b':' if depth == 1 => {
                    let key = pending
                        .take()
                        .ok_or_else(|| format!("`{field}` 里有个 `:` 前面不是字符串键"))?;
                    if keys.contains(&key) {
                        return Err(format!("`{field}` 里键 `{key}` 出现了两次"));
                    }
                    keys.push(key);
                    i += 1;
                }
                b'{' | b'[' => {
                    depth += 1;
                    pending = None;
                    i += 1;
                }
                b'}' | b']' => {
                    depth -= 1;
                    if depth == 0 {
                        if bytes[i] != b'}' {
                            return Err(format!("`{field}` 的对象被 `]` 关掉了"));
                        }
                        if keys.is_empty() {
                            return Err(format!("`{field}` 是个空对象"));
                        }
                        return Ok(keys);
                    }
                    pending = None;
                    i += 1;
                }
                b',' => {
                    pending = None;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        Err(format!("`{field}` 的对象没有闭合"))
    }

    /// 读 `src[start..]` 处的一个 JSON 字符串字面量，返回 `(内容, 右引号之后的下标)`。
    ///
    /// 不解转义：manifest 的平台 key 与 sha 都是朴素 ASCII，遇到转义一律 `Err` ——
    /// 猜错的表现是一个平台名被悄悄改写，那比红一次贵得多。
    fn json_string(src: &str, start: usize) -> Result<(String, usize), String> {
        let bytes = src.as_bytes();
        let mut i = start + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    let text = src
                        .get(start + 1..i)
                        .ok_or_else(|| "字符串跨了 UTF-8 边界".to_owned())?;
                    return Ok((text.to_owned(), i + 1));
                }
                b'\\' => {
                    return Err("字符串里有转义，本读取器只认朴素 ASCII 键".to_owned());
                }
                _ => i += 1,
            }
        }
        Err("字符串没有闭合".to_owned())
    }

    // ── 最小 protobuf wire 读取 ────────────────────────────────────────────────

    fn read_varint(b: &[u8], i: &mut usize) -> Option<u64> {
        let (mut out, mut shift) = (0u64, 0u32);
        loop {
            let c = *b.get(*i)?;
            *i += 1;
            out |= u64::from(c & 0x7F) << shift;
            if c & 0x80 == 0 {
                return Some(out);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }

    /// 逐字段扫一个完整 buffer，产出 `(字段号, wire type, 值切片/varint)`。
    /// 只需要 varint(0) 与 length-delimited(2)；descriptor 里不出现其它 wire type，遇到即判定越界停下。
    fn each_field(b: &[u8], mut f: impl FnMut(u32, u8, &[u8], u64)) -> bool {
        let mut i = 0usize;
        while i < b.len() {
            let Some(key) = read_varint(b, &mut i) else {
                return false;
            };
            let (num, wt) = ((key >> 3) as u32, (key & 7) as u8);
            if num == 0 {
                return false;
            }
            match wt {
                0 => {
                    let Some(v) = read_varint(b, &mut i) else {
                        return false;
                    };
                    f(num, wt, &[], v);
                }
                2 => {
                    let Some(len) = read_varint(b, &mut i) else {
                        return false;
                    };
                    let end = i.saturating_add(len as usize);
                    if end > b.len() {
                        return false;
                    }
                    f(num, wt, &b[i..end], 0);
                    i = end;
                }
                _ => return false,
            }
        }
        true
    }

    /// 一份 rawDesc 解出的全部符号表。
    ///
    /// `messages`：message 名 → (字段名 → 字段号)。
    /// `enums`：enum 名 → (值名 → 值号)。**枚举值号同属 wire 契约**：`DefaultLogLevel` 的全部载荷
    /// 就是一个枚举数字，值序漂了就是把 warn 显示成 info——比不显示更糟。
    #[derive(Debug, Default, Clone)]
    pub struct CoreDescriptor {
        pub messages: BTreeMap<String, BTreeMap<String, u32>>,
        pub enums: BTreeMap<String, BTreeMap<String, u32>>,
    }

    /// 从二进制里定位 rawDesc 并解析出全部 message / enum 符号表。
    ///
    /// rawDesc 在 Go 二进制里没有自带长度前缀（长度存在别处的 string header 里），故从锚点起**贪心**
    /// 逐字段解析，遇到不像 `FileDescriptorProto` 的 tag 即停——与逐个二进制实测出的边界一致
    /// （linux/mac-arm64 的 beta.7、以及 alpha.40 三份都在此策略下解出完整 message 表）。
    pub fn descriptor_from_core(binary: &[u8]) -> Result<CoreDescriptor, String> {
        let mut needle = Vec::with_capacity(PROTO_FILE_NAME.len() + 2);
        needle.push(0x0A);
        needle.push(PROTO_FILE_NAME.len() as u8);
        needle.extend_from_slice(PROTO_FILE_NAME);

        let start = binary
            .windows(needle.len())
            .position(|w| w == needle)
            .ok_or_else(|| {
                format!(
                    "在二进制里找不到 `{}` 的 FileDescriptorProto 锚点 —— \
                     该核可能不是 sing-box、或上游改了 daemon proto 的文件路径。",
                    String::from_utf8_lossy(PROTO_FILE_NAME)
                )
            })?;

        // 贪心切一段足够大的窗口；`each_field` 自己会在非法 tag 处停。
        let window = &binary[start..binary.len().min(start + (1 << 20))];
        let mut out = CoreDescriptor::default();
        let mut i = 0usize;
        while i < window.len() {
            let mut j = i;
            let Some(key) = read_varint(window, &mut j) else {
                break;
            };
            let (num, wt) = ((key >> 3) as u32, (key & 7) as u8);
            // FileDescriptorProto 的合法 tag 集；越界即认为 rawDesc 到头了。
            if !matches!(num, 1..=9 | 12) || !matches!(wt, 0 | 2) {
                break;
            }
            let payload: &[u8] = if wt == 2 {
                let Some(len) = read_varint(window, &mut j) else {
                    break;
                };
                let end = j.saturating_add(len as usize);
                if end > window.len() {
                    break;
                }
                let p = &window[j..end];
                j = end;
                p
            } else {
                if read_varint(window, &mut j).is_none() {
                    break;
                }
                &[]
            };
            // FileDescriptorProto：4=message_type, 5=enum_type。
            match num {
                4 => collect_message(payload, "", &mut out),
                5 => collect_enum(payload, "", &mut out.enums),
                _ => {}
            }
            i = j;
        }

        if out.messages.is_empty() {
            return Err(
                "找到了锚点但没解析出任何 message —— rawDesc 结构可能已变，请重核解析策略。".into(),
            );
        }
        Ok(out)
    }

    /// 递归收集 `DescriptorProto`（field 1=name, 2=field, 3=nested_type, 4=enum_type）。
    fn collect_message(buf: &[u8], prefix: &str, out: &mut CoreDescriptor) {
        let mut name = String::new();
        let mut fields: Vec<Vec<u8>> = Vec::new();
        let mut nested: Vec<Vec<u8>> = Vec::new();
        let mut nested_enums: Vec<Vec<u8>> = Vec::new();
        each_field(buf, |num, wt, val, _| {
            if wt != 2 {
                return;
            }
            match num {
                1 => name = String::from_utf8_lossy(val).into_owned(),
                2 => fields.push(val.to_vec()),
                3 => nested.push(val.to_vec()),
                4 => nested_enums.push(val.to_vec()),
                _ => {}
            }
        });
        if name.is_empty() {
            return;
        }
        let full = format!("{prefix}{name}");
        let mut map = BTreeMap::new();
        for fb in &fields {
            // FieldDescriptorProto：field 1=name(string), 3=number(varint)。
            let (mut fname, mut fnum) = (String::new(), 0u32);
            each_field(fb, |num, wt, val, v| match (num, wt) {
                (1, 2) => fname = String::from_utf8_lossy(val).into_owned(),
                (3, 0) => fnum = v as u32,
                _ => {}
            });
            if !fname.is_empty() && fnum != 0 {
                map.insert(fname, fnum);
            }
        }
        out.messages.insert(full.clone(), map);
        for n in &nested {
            collect_message(n, &format!("{full}."), out);
        }
        for e in &nested_enums {
            collect_enum(e, &format!("{full}."), &mut out.enums);
        }
    }

    /// 收集 `EnumDescriptorProto`（field 1=name, 2=value）。
    ///
    /// 值号用 varint 且**合法取 0**（`PANIC = 0`），故不能像字段号那样拿 `0` 当「没读到」丢掉：
    /// 缺 `number` 字段按 protobuf 语义即默认值 0，与「显式写 0」同义。
    fn collect_enum(buf: &[u8], prefix: &str, out: &mut BTreeMap<String, BTreeMap<String, u32>>) {
        let mut name = String::new();
        let mut values: Vec<Vec<u8>> = Vec::new();
        each_field(buf, |num, wt, val, _| match (num, wt) {
            (1, 2) => name = String::from_utf8_lossy(val).into_owned(),
            (2, 2) => values.push(val.to_vec()),
            _ => {}
        });
        if name.is_empty() {
            return;
        }
        let mut map = BTreeMap::new();
        for vb in &values {
            // EnumValueDescriptorProto：field 1=name(string), 2=number(varint)。
            let (mut vname, mut vnum) = (String::new(), 0u32);
            each_field(vb, |num, wt, val, v| match (num, wt) {
                (1, 2) => vname = String::from_utf8_lossy(val).into_owned(),
                (2, 0) => vnum = v as u32,
                _ => {}
            });
            if !vname.is_empty() {
                map.insert(vname, vnum);
            }
        }
        out.insert(format!("{prefix}{name}"), map);
    }

    // ── vendored .proto 文本解析 ───────────────────────────────────────────────

    /// 从 `.proto` 源文本里取某个 message 的 `字段名 → 字段号`。
    ///
    /// `message` 支持**带点的嵌套全名**（`Log.Message`），与 descriptor 侧
    /// [`collect_message`] 产出的全名同一套词汇 —— 否则 `Log.Message` 这类嵌套消息只能靠拍平成顶层
    /// 才进得了对拍表，而拍平之后真核 descriptor 里点名不到它，那格覆盖面就是空的。
    ///
    /// 只认字段声明（`<type> <name> = <num>;`，可带 `repeated`/`optional` 与 map 类型）。嵌套
    /// message/enum 块整段跳过（由带点路径单独对拍），但 oneof 内字段属于父 message，必须收进来。
    /// 行扫而非写完整文法：目标是让**门自身足够笨**——门比被测对象复杂就轮到门自己出错了。
    pub fn message_from_proto_src(
        src: &str,
        message: &str,
    ) -> Result<BTreeMap<String, u32>, String> {
        let mut body: Vec<&str> = src.lines().collect();
        for seg in message.split('.') {
            body = block_body(&body, &format!("message {seg} {{")).ok_or_else(|| {
                format!("vendored proto 里找不到 `message {seg}`（对拍名 `{message}`）")
            })?;
        }
        fields_from_block(&body, message)
    }

    /// 取 `header` 那一行之后、到与之配对的 `}` 之前的全部行（不含首尾两行本身）。
    /// 花括号计数只看代码部分（行注释先剥掉），故注释里的括号不会把配对算歪。
    fn block_body<'a>(lines: &[&'a str], header: &str) -> Option<Vec<&'a str>> {
        let start = lines.iter().position(|l| l.trim() == header)?;
        let mut depth = 1usize;
        let mut out = Vec::new();
        for line in &lines[start + 1..] {
            let code = line.split("//").next().unwrap_or("");
            depth += code.matches('{').count();
            let closes = code.matches('}').count();
            if closes >= depth {
                return Some(out); // 本块闭合
            }
            depth -= closes;
            out.push(*line);
        }
        None // 花括号没闭合
    }

    /// 一个 message 块体 → `字段名 → 字段号`。嵌套 message/enum 跳过，oneof 字段保留。
    fn fields_from_block(body: &[&str], message: &str) -> Result<BTreeMap<String, u32>, String> {
        let mut out = BTreeMap::new();
        let mut skip_depth = 0usize;
        let mut oneof_depth = 0usize;
        for line in body {
            let code = line.split("//").next().unwrap_or("").trim();
            if code.is_empty() {
                continue;
            }
            if skip_depth > 0 {
                skip_depth += code.matches('{').count();
                skip_depth -= code.matches('}').count().min(skip_depth);
                continue;
            }
            if code.starts_with("oneof ") && code.ends_with('{') {
                oneof_depth += 1;
                continue;
            }
            if oneof_depth > 0 && code == "}" {
                oneof_depth -= 1;
                continue;
            }
            if code.ends_with('{') {
                skip_depth = 1; // 嵌套块：本层不收，带点路径单独对拍
                continue;
            }
            if code.starts_with("reserved ") {
                continue;
            }
            let mut tok: Vec<&str> = code.split_whitespace().collect();
            if tok.first() == Some(&"repeated") || tok.first() == Some(&"optional") {
                tok.remove(0);
            }
            // 期望尾部形态：... <name> = <num>;。`map<string, string>` 的类型本身会拆成两段。
            if tok.len() < 4 || tok[tok.len() - 2] != "=" {
                return Err(format!(
                    "vendored proto 里这行看不懂（门只认字段声明）：{code}"
                ));
            }
            let num: u32 = tok[tok.len() - 1]
                .trim_end_matches(';')
                .parse()
                .map_err(|_| format!("字段号不是数字：{code}"))?;
            out.insert(tok[tok.len() - 3].to_string(), num);
        }
        if out.is_empty() {
            return Err(format!("`message {message}` 里一个字段都没解出来"));
        }
        Ok(out)
    }

    /// 从 `.proto` 源文本里取某个 enum 的 `值名 → 值号`。同 [`message_from_proto_src`] 的取舍：
    /// 只认最朴素的 `NAME = <num>;`，门自身足够笨。
    pub fn enum_from_proto_src(src: &str, name: &str) -> Result<BTreeMap<String, u32>, String> {
        let header = format!("enum {name} {{");
        let mut lines = src.lines().skip_while(|l| l.trim() != header.trim());
        lines
            .next()
            .ok_or_else(|| format!("vendored proto 里找不到 `enum {name}`"))?;

        let mut out = BTreeMap::new();
        for line in lines {
            let code = line.split("//").next().unwrap_or("").trim();
            if code == "}" {
                return Ok(out);
            }
            if code.is_empty() {
                continue;
            }
            // 期望形态：<NAME> = <num>;
            let tok: Vec<&str> = code.split_whitespace().collect();
            if tok.len() != 3 || tok[1] != "=" {
                return Err(format!(
                    "vendored proto 里这行看不懂（门只认最朴素的枚举值声明）：{code}"
                ));
            }
            let num: u32 = tok[2]
                .trim_end_matches(';')
                .parse()
                .map_err(|_| format!("枚举值号不是数字：{code}"))?;
            out.insert(tok[0].to_string(), num);
        }
        Err(format!("`enum {name}` 的花括号没闭合"))
    }

    /// 被对拍的符号种类。message 与 enum 的取证路径不同（descriptor 里分别是 field 4 / field 5），
    /// 但判据完全一样：**vendored 声明的每一项，号必须与真核一致**。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SymbolKind {
        Message,
        Enum,
    }

    impl SymbolKind {
        fn label(self) -> &'static str {
            match self {
                SymbolKind::Message => "message",
                SymbolKind::Enum => "enum",
            }
        }
    }

    /// vendored proto 文本里某个 message/enum 的号表。
    pub fn symbol_from_proto_src(
        src: &str,
        kind: SymbolKind,
        name: &str,
    ) -> Result<BTreeMap<String, u32>, String> {
        match kind {
            SymbolKind::Message => message_from_proto_src(src, name),
            SymbolKind::Enum => enum_from_proto_src(src, name),
        }
    }

    // ── 对拍 ──────────────────────────────────────────────────────────────────

    /// 判据：**vendored 声明的每一项（message 字段 / enum 值），号必须与真核一致**。
    ///
    /// 只做单向（vendored ⊆ real）而不要求两侧集合相等 —— 上游新增我们不消费的字段是常态、无害；
    /// 而「我们声明了、但号对不上」才是会把整帧解崩的那一类。返回全部不一致项，不在第一条就短路：
    /// 字段号漂移往往是**成片**的（插一个字段后面全 +1），只报第一条会让人误以为是孤立笔误。
    pub fn diff(vendored: &BTreeMap<String, u32>, real: &BTreeMap<String, u32>) -> Vec<String> {
        let mut bad = Vec::new();
        for (name, num) in vendored {
            match real.get(name) {
                Some(r) if r == num => {}
                Some(r) => bad.push(format!("  {name}: vendored={num} 真核={r}")),
                None => bad.push(format!("  {name}: vendored={num} 真核**无此项**")),
            }
        }
        bad
    }

    /// 一次完整对拍：读核 → 抠 descriptor → 与 vendored proto 文本比某个 message/enum。
    /// `Ok(())` = 一致；`Err(人类可读报告)` = 不一致或提取失败。
    pub fn check_core_against_proto(
        core: &Path,
        proto_src: &str,
        kind: SymbolKind,
        name: &str,
    ) -> Result<(), String> {
        let bytes =
            std::fs::read(core).map_err(|e| format!("读不到随包核 {}：{e}", core.display()))?;
        let desc =
            descriptor_from_core(&bytes).map_err(|e| format!("{}：{e}", core.display()))?;
        let table = match kind {
            SymbolKind::Message => &desc.messages,
            SymbolKind::Enum => &desc.enums,
        };
        let kind_label = kind.label();
        let real = table.get(name).ok_or_else(|| {
            format!(
                "{} 的 descriptor 里没有 {kind_label} `{name}`",
                core.display()
            )
        })?;
        let vendored = symbol_from_proto_src(proto_src, kind, name)?;
        let bad = diff(&vendored, real);
        if bad.is_empty() {
            return Ok(());
        }
        Err(format!(
            "vendored proto 与随包内核的 wire 契约不一致：{}\n\
             {kind_label} `{name}` 的号对不上：\n{}\n\n\
             后果（2026-08-05 真机实证）：prost 对 wire type 不匹配零容忍，错位后**整帧解码失败**，\n\
             而 ReconnectingStream 把它当断线无限重连 —— 外部只看得到「没有下一帧」，功能整块静默消失。\n\
             修复：以随包核的 descriptor 为准改 `crates/singbox-grpc/proto/started_service.proto`，\n\
             不要以文档或旧版本记忆为准。真核字段号可直接读出：\n\
               strings -n 6 {} | grep -oE 'protobuf:\"[^\"]*\"' | sort -u",
            core.display(),
            bad.join("\n"),
            core.display(),
        ))
    }

    // ── 共享真值：符号表 + vendored proto 文本 ────────────────────────────────
    //
    // 这两项此前在 `build.rs` 与 `tests/bundled_core_wire.rs` 各存一份，两处注释都写着
    // 「一处漏加，另一处就白守」——那是一个已知的漂移源，且随着运行期也要用它而变成三份。
    // 下沉到本文件后三个消费点共用一份。
    //
    // `include_str!` 的相对路径按**本文件所在目录**解析（`crates/singbox-grpc/`），
    // 故三个 include 点（build.rs / tests / lib）拿到的是同一份文本。

    /// vendored proto 原文。
    pub const PROTO_SRC: &str = include_str!("proto/started_service.proto");

    /// 必须与真核 wire 一致的符号表。**新增的消费面必须进表**（理由见 `build.rs` 的
    /// `assert_proto_matches_bundled_core` 文档：立手法优先于铺覆盖面）。
    pub const CHECKED_SYMBOLS: &[(SymbolKind, &str)] = &[
        (SymbolKind::Message, "TailscaleEndpointStatus"),
        (SymbolKind::Message, "TailscaleUserGroup"),
        (SymbolKind::Message, "TailscalePeer"),
        (SymbolKind::Message, "OpenConnectStatusUpdate"),
        (SymbolKind::Message, "OpenConnectEndpointStatus"),
        (SymbolKind::Message, "OpenConnectTunnelInfo"),
        (SymbolKind::Message, "OpenConnectAuthChallenge"),
        (SymbolKind::Message, "OpenConnectAuthForm"),
        (SymbolKind::Message, "OpenConnectAuthFormField"),
        (SymbolKind::Message, "OpenConnectAuthFormChoice"),
        (SymbolKind::Message, "OpenConnectBrowserRequest"),
        (SymbolKind::Message, "OpenConnectBrowserCookie"),
        (SymbolKind::Message, "OpenConnectBrowserHeader"),
        (SymbolKind::Message, "OpenConnectAuthFormResponse"),
        (SymbolKind::Message, "OpenConnectBrowserResult"),
        (SymbolKind::Message, "OpenConnectAuthResponseSubmission"),
        (SymbolKind::Message, "OpenConnectAuthChallengeCancel"),
        (SymbolKind::Message, "OpenVPNStatusUpdate"),
        (SymbolKind::Message, "OpenVPNEndpointStatus"),
        (SymbolKind::Message, "OpenVPNTunnelInfo"),
        (SymbolKind::Message, "OpenVPNChallenge"),
        (SymbolKind::Message, "OpenVPNChallengeSubmission"),
        (SymbolKind::Message, "OpenVPNChallengeCancel"),
        (SymbolKind::Message, "DefaultLogLevel"),
        (SymbolKind::Enum, "LogLevel"),
        (SymbolKind::Message, "Log"),
        (SymbolKind::Message, "Log.Message"),
        // Taildrop 收件侧的全部消费面（1.14.0-beta.15）。进表的理由同 `Log`：这几个的字段撞号
        // 不会报错，只会静默失真 —— `TaildropFile.size/modifiedAt` 都是 int64、`senderName/name`
        // 都是 string，互换后收件箱照常渲染，只是把文件名当成发件人、把时间当成大小。
        // `TaildropReceivingFile` 更硬：`senderID` + `name` 是 `CancelTaildropReceiving` 的定位键，
        // 撞号 = 取消到别的文件上去。
        (SymbolKind::Message, "TaildropInbox"),
        (SymbolKind::Message, "TaildropFile"),
        (SymbolKind::Message, "TaildropReceivingFile"),
        (SymbolKind::Message, "DownloadTaildropFileChunk"),
    ];

    /// 对**内存里的一份核**做整表对拍的三态结论。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WireVerdict {
        /// 表里每个符号的字段号都对得上。
        Match,
        /// **有字段号对不上** —— 唯一「危险到该拦」的一档，附人类可读报告。
        Mismatch(String),
        /// 取不到判据（抠不出 descriptor / 表里的符号在该核里不存在 / vendored 侧解析失败）。
        Unobservable(String),
    }

    /// 整表对拍一份**内存中的**核（不落盘、不需要路径）。
    ///
    /// # 为什么「符号缺失」与「提取失败」都判 `Unobservable` 而不是 `Mismatch`
    ///
    /// 两档的**失败响度**不同，而这个结论会被用来拦一次用户主动发起的换核：
    ///
    /// - **字段号对不上** ⇒ prost 整帧解码失败 ⇒ `ReconnectingStream` 当断线无限重连 ⇒
    ///   功能整块**静默**消失（2026-08-05 真机实证）。用户看不出，必须拦。
    /// - **符号根本不在** ⇒ 对应 rpc 直接 `Unimplemented` / 空表，失败是**响亮**的。
    /// - **抠不出 descriptor** ⇒ 我们对这份核一无所知 —— 「没观测到 ≠ 观测到没问题」，
    ///   据此拦下用户自选的核属于把一次读失败升级成一次功能剥夺。
    ///
    /// 故只有第一档拦，其余两档放行 + 大声告警。判据方向与 `runtime::proxy::process_supervision::pid_identity_verdict`
    /// 一致（那里同样是「取不到材料一律 Unobservable，绝不折成不匹配」）。
    pub fn verdict_for_core_bytes(bytes: &[u8]) -> WireVerdict {
        let desc = match descriptor_from_core(bytes) {
            Ok(d) => d,
            Err(e) => return WireVerdict::Unobservable(e),
        };
        let mut bad = Vec::new();
        for (kind, name) in CHECKED_SYMBOLS {
            let table = match kind {
                SymbolKind::Message => &desc.messages,
                SymbolKind::Enum => &desc.enums,
            };
            let Some(real) = table.get(*name) else {
                return WireVerdict::Unobservable(format!(
                    "该核的 descriptor 里没有 {} `{name}`",
                    kind.label()
                ));
            };
            let vendored = match symbol_from_proto_src(PROTO_SRC, *kind, name) {
                Ok(v) => v,
                Err(e) => return WireVerdict::Unobservable(e),
            };
            for line in diff(&vendored, real) {
                bad.push(format!("{} `{name}`{line}", kind.label()));
            }
        }
        if bad.is_empty() {
            WireVerdict::Match
        } else {
            WireVerdict::Mismatch(bad.join("\n"))
        }
    }
}

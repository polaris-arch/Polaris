//! 本地导入 IPC（`local_import_parse` → [`parse_local_import_owned`]）的**端到端产物**门。
//!
//! 桌面与移动端（Android）走的是同一个 Tauri command、同一条解析路径（`lib.rs` 的
//! `invoke_handler` 只有一份、本命令与 `singbox_import` 都没有 `cfg` 分叉）。net-stack 那侧的
//! 单测直调 `parse_subscription`，证明的是解析器；这里钉的是**命令层真正交给前端的 JSON**：
//! MASQUE / Tailcat 节点在 `nodes` 里、生成侧会拒收的计进 `stats.failed`、证书固定提示进
//! `warnings`、reality 节点上内核不校验的 pin 不存。移动端导入预览渲染的正是这几个字段。

use super::super::parse_local_import_owned;
use serde_json::{json, Value};

const TC_PUB: &str = "lPLDHP0YorENQouqgSUx1GHu+3OcDc/F71Z3roMTSy4=";
const TC_DISCO: &str = "qQ+kiWwZ8BrTYDZpj+6bnx2JxWxx0SAh1krqPGndCmQ=";
const PIN_HEX: &str = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881";

fn run(doc: &Value) -> Value {
    let resp = parse_local_import_owned(doc.to_string());
    assert!(resp.success, "本地导入失败：{:?}", resp.error);
    resp.data.expect("成功信封没有 data")
}

fn node<'a>(data: &'a Value, name: &str) -> &'a Value {
    data["nodes"]
        .as_array()
        .expect("nodes 不是数组")
        .iter()
        .find(|n| n["name"] == name)
        .unwrap_or_else(|| panic!("产物里没有节点 {name}：{}", data["nodes"]))
}

fn pin_warnings(data: &Value) -> Vec<String> {
    data["warnings"]
        .as_array()
        .expect("warnings 不是数组")
        .iter()
        .filter_map(Value::as_str)
        .filter(|w| w.contains("证书固定"))
        .map(str::to_owned)
        .collect()
}

fn doc() -> Value {
    json!({
        "endpoints": [
            { "type": "masque-client", "tag": "MQ", "server": "mq.example.com", "server_port": 443,
              "path": "/.well-known/masque/ip/" },
            // 生成侧会拒收（path 不以 / 开头）⇒ 计 failed，不入库。
            { "type": "masque-client", "tag": "MQ-BAD", "server": "mq.example.com", "server_port": 443,
              "path": "no-slash" }
        ],
        "outbounds": [
            { "type": "tailcat", "tag": "TC", "server_public_key": TC_PUB,
              "server_disco_key": TC_DISCO, "derp_region": 900 },
            { "type": "trojan", "tag": "TJ", "server": "t.example.com", "server_port": 443,
              "password": "p",
              "tls": { "enabled": true, "server_name": "t.example.com", "certificate_sha256": PIN_HEX } },
            { "type": "vless", "tag": "RL", "server": "r.example.com", "server_port": 443, "uuid": "u",
              "tls": { "enabled": true, "server_name": "sni", "certificate_sha256": PIN_HEX,
                       "reality": { "enabled": true, "public_key": "PBK", "short_id": "ab" } } }
        ]
    })
}

/// MASQUE / Tailcat 经本地导入命令产出对应节点；坏 MASQUE 计 failed。
#[test]
fn local_import_ipc_yields_masque_and_tailcat_nodes() {
    let data = run(&doc());
    assert_eq!(node(&data, "MQ")["protocol"], "masque-client");
    assert_eq!(node(&data, "TC")["protocol"], "tailcat");
    assert!(
        data["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["name"] != "MQ-BAD"),
        "生成侧会拒收的 MASQUE 节点进了预览"
    );
    assert_eq!(
        data["stats"]["failed"], 1,
        "坏 MASQUE 节点没计进 failed：{}",
        data["stats"]
    );
    assert_eq!(data["format"], "singbox");
}

/// 证书固定：会下发的 pin（trojan）计进提示并保留；reality 上内核不校验的 pin 不存、也不计数。
#[test]
fn local_import_ipc_warns_about_emitted_pins_and_drops_ignored_ones() {
    let data = run(&doc());
    let w = pin_warnings(&data);
    assert_eq!(w.len(), 1, "证书固定提示应恰好一条：{:?}", data["warnings"]);
    assert!(
        w[0].starts_with("1 个节点"),
        "计数口径应只含会下发的那一个：{}",
        w[0]
    );
    assert_eq!(
        node(&data, "TJ")["tlsSettings"]["certificateSha256"],
        PIN_HEX
    );
    assert!(
        node(&data, "RL")["tlsSettings"]
            .get("certificateSha256")
            .is_none(),
        "reality 节点的 pin 内核不校验，不该存：{}",
        node(&data, "RL")
    );
}

/// 反向对照：没有任何 pin 的同一批节点不出提示（证明上面那条不是恒出）。
#[test]
fn local_import_ipc_no_pin_no_warning() {
    let mut d = doc();
    for ob in d["outbounds"].as_array_mut().unwrap() {
        if let Some(tls) = ob.get_mut("tls").and_then(Value::as_object_mut) {
            tls.remove("certificate_sha256");
        }
    }
    let data = run(&d);
    assert!(pin_warnings(&data).is_empty(), "{:?}", data["warnings"]);
    assert_eq!(node(&data, "TC")["protocol"], "tailcat");
}

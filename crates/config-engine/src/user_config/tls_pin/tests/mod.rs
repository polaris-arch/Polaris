use super::*;

// 参照值由 coreutils/openssl 独立算出（不是用本模块自己算的）：
// `printf x | sha256sum` / `printf x | openssl dgst -sha256 -binary | base64`。
const X_HEX: &str = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881";
const X_B64: &str = "LXEWQrcmsEQBYnyp+6wy9chTD7GQPMTbAiWHF5IaSIE=";
const EMPTY_HEX: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const EMPTY_B64: &str = "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=";

fn colon(hex: &str) -> String {
    hex.as_bytes()
        .chunks(2)
        .map(|c| std::str::from_utf8(c).unwrap().to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(":")
}

#[test]
fn hex_colon_hex_and_base64_all_land_on_the_same_kernel_form() {
    let want = Some(vec![X_B64.to_string()]);
    assert_eq!(cert_pins_for_kernel(Some(X_HEX)), want, "裸 hex");
    assert_eq!(
        cert_pins_for_kernel(Some(&X_HEX.to_ascii_uppercase())),
        want,
        "大写 hex"
    );
    assert_eq!(
        cert_pins_for_kernel(Some(&colon(X_HEX))),
        want,
        "openssl 冒号 hex"
    );
    assert_eq!(
        cert_pins_for_kernel(Some(&colon(X_HEX).replace(':', "-"))),
        want,
        "hysteria 的 - 分隔"
    );
    assert_eq!(cert_pins_for_kernel(Some(X_B64)), want, "标准 base64 原样");
    assert_eq!(
        cert_pins_for_kernel(Some(&format!("  {X_HEX}  "))),
        want,
        "首尾空白"
    );
}

#[test]
fn comma_list_keeps_order_and_drops_only_the_bad_items() {
    assert_eq!(
        cert_pins_for_kernel(Some(&format!("{X_HEX}, chrome ,{EMPTY_HEX},"))),
        Some(vec![X_B64.to_string(), EMPTY_B64.to_string()])
    );
}

#[test]
fn illegal_inputs_never_reach_the_kernel() {
    for bad in [
        "",
        "   ",
        "chrome",                       // mihomo 把浏览器名误填进 fingerprint 的老配置
        &X_HEX[..62],                   // 少一个字节
        &format!("{X_HEX}00"),          // 多一个字节
        &format!("{}zz", &X_HEX[..62]), // 非 hex 字符
        "AAAA",                         // 合法 base64 但只有 3 字节 —— 内核 check 照收，本门必须拒
        &X_B64.replace('=', ""),        // 无填充
        &X_B64.replace('+', "-").replace('/', "_"), // url-safe
        "AA==",
    ] {
        assert_eq!(
            cert_pins_for_kernel(Some(bad)),
            None,
            "{bad:?} 不应产生任何 pin"
        );
    }
    assert_eq!(cert_pins_for_kernel(None), None);
}

#[test]
fn import_side_keeps_original_text_of_valid_items_only() {
    assert_eq!(
        keep_valid_cert_pins(&colon(X_HEX)),
        Some(colon(X_HEX)),
        "原文保留，不转码"
    );
    assert_eq!(
        keep_valid_cert_pins(&format!(" {X_HEX} ,chrome,{X_B64}")),
        Some(format!("{X_HEX},{X_B64}"))
    );
    assert_eq!(keep_valid_cert_pins("chrome"), None);
    assert_eq!(keep_valid_cert_pins(""), None);
}

#[test]
fn base64_codec_round_trips_every_padding_shape() {
    for len in 0..=34usize {
        let bytes: Vec<u8> = (0..len as u8)
            .map(|b| b.wrapping_mul(37).wrapping_add(11))
            .collect();
        let enc = encode_std_base64(&bytes);
        assert_eq!(enc.len(), len.div_ceil(3) * 4, "len={len}");
        if len > 0 {
            assert_eq!(decode_std_base64(&enc), Some(bytes), "len={len} enc={enc}");
        }
    }
    assert_eq!(encode_std_base64(&[0]), "AA==");
    assert_eq!(encode_std_base64(&[0, 0]), "AAA=");
}

fn server(json: &str) -> crate::user_config::server_config::ServerConfig {
    serde_json::from_str(json).expect("server json")
}

/// 导入提示只数会真正下发 pin 的节点：reality / naive 不计，无 pin 不提示。
#[test]
fn import_warning_counts_only_nodes_whose_pin_is_emitted() {
    let pinned = format!(
        r#"{{"id":"a","name":"a","protocol":"trojan","address":"a.com","port":443,"password":"p",
            "security":"tls","tlsSettings":{{"certificateSha256":"{X_HEX}"}}}}"#
    );
    let reality = format!(
        r#"{{"id":"b","name":"b","protocol":"vless","address":"a.com","port":443,"uuid":"u",
            "security":"reality","tlsSettings":{{"certificateSha256":"{X_HEX}"}}}}"#
    );
    let plain = r#"{"id":"c","name":"c","protocol":"trojan","address":"a.com","port":443,"password":"p","security":"tls"}"#;
    let w = cert_pin_import_warning(&[server(&pinned), server(&reality), server(plain)])
        .expect("pinned node must warn");
    assert!(w.starts_with("1 个节点"), "{w}");
    assert!(cert_pin_import_warning(&[server(&reality), server(plain)]).is_none());
}

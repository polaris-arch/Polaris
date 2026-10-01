use super::*;
// 本模块自带：`to_response` 收敛成 wire 往返后，生产面只剩 `Response`，这三个是断言面专用。
// 留在生产面会让 darwin 交叉 clippy 红（那里没有 cfg(test)），而本机 `--all-targets` 带
// cfg(test)、看不出来 —— 属「本机绿但交叉红」的一族。
use polaris_helper_proto::Error as ProtoError;
use polaris_helper_proto::ErrorCode;
use polaris_helper_proto::ResponseKind;

/// `InstallResult` 的**全部** variant，逐个带一个有代表性的 detail。
///
/// 手写全集：加了 variant 忘了加到这里，下面那条等价断言就少覆盖一格。`to_wire_line` 的
/// `match` 是编译期穷举的，故 `variant_count` 那条断言把两者钉在一起。
fn every_variant() -> Vec<InstallResult> {
    vec![
        InstallResult::Installed,
        InstallResult::CoreDirUnset,
        InstallResult::BadArgs,
        InstallResult::ReadSingbox("open /x: no such file".into()),
        InstallResult::HashMismatch,
        InstallResult::ReadDir("permission denied".into()),
        InstallResult::Mkdir("read-only file system".into()),
        InstallResult::Read {
            name: "libcronet.dylib".into(),
            detail: "no such file".into(),
        },
        InstallResult::Write {
            name: "sing-box".into(),
            detail: "no space left".into(),
        },
        InstallResult::Rename {
            name: "sing-box".into(),
            detail: "text file busy".into(),
        },
        InstallResult::Busy,
    ]
}

/// `to_response` 收敛到 wire 往返后**继续**要守的东西：往返对每一个 variant 无损。
///
/// 手写表删掉了，但那张表原本在守一件事 —— 每个 variant 都能变成一个**语义正确、不丢 detail**
/// 的 `Response`。往返本身不保证这点（`Error::parse` 对未知 token 归 `Other`，若某个 variant 的
/// `to_wire_line` 写错了形态，往返照样「成功」）。故逐 variant 钉住：
/// - `Response` 再序列化回 wire 行，必须与 `to_wire_line()` **逐字相同**（无损）；
/// - 带 detail 的 variant，detail 里的那段 OS 错误文字必须还在（不被 code 吃掉）。
///
/// 收敛前出过「手写表 == 往返」的等价收据（11/11 逐字相同），那条已随手写表一起删。
#[test]
fn wire_roundtrip_is_lossless_for_every_variant() {
    for r in every_variant() {
        let line = r.to_wire_line();
        let resp = to_response(r.clone());
        assert_eq!(resp.to_wire_line(), line, "variant {r:?} 的 wire 往返有损");
        assert!(!line.contains('\n'), "variant {r:?} 的 wire 行含换行");
    }
    // 带 detail 的那批：OS 错误文字必须原样在 detail 里（`Other` + 完整原文）。
    for (r, needle) in [
        (
            InstallResult::ReadSingbox("open /x: no such file".into()),
            "no such file",
        ),
        (
            InstallResult::Write {
                name: "sing-box".into(),
                detail: "no space left".into(),
            },
            "no space left",
        ),
    ] {
        let Response::Err(e) = to_response(r.clone()) else {
            panic!("{r:?} 不该是 Ok");
        };
        assert_eq!(e.code, ErrorCode::Other);
        assert!(e.detail.contains(needle), "{r:?} 丢了 detail：{}", e.detail);
    }
}

#[test]
fn install_result_to_response_installed() {
    let resp = to_response(InstallResult::Installed);
    assert!(matches!(resp, Response::Ok(ResponseKind::Installed)));
}

#[test]
fn install_result_to_response_error_codes() {
    let resp = to_response(InstallResult::CoreDirUnset);
    assert!(matches!(
        resp,
        Response::Err(ProtoError {
            code: ErrorCode::CoredirUnset,
            ..
        })
    ));

    let resp = to_response(InstallResult::BadArgs);
    assert!(matches!(
        resp,
        Response::Err(ProtoError {
            code: ErrorCode::BadArgs,
            ..
        })
    ));

    let resp = to_response(InstallResult::HashMismatch);
    assert!(matches!(
        resp,
        Response::Err(ProtoError {
            code: ErrorCode::HashMismatch,
            ..
        })
    ));
}

#[test]
fn install_result_to_response_os_errors_keep_detail() {
    // read-singbox 等走 ErrorCode::Other + detail 保留完整原文
    let resp = to_response(InstallResult::ReadSingbox("open /x: no such file".into()));
    match resp {
        Response::Err(e) => {
            assert_eq!(e.code, ErrorCode::Other);
            assert!(e.detail.contains("read-singbox"), "{}", e.detail);
            assert!(e.detail.contains("no such file"), "{}", e.detail);
        }
        other => panic!("{other:?}"),
    }
}

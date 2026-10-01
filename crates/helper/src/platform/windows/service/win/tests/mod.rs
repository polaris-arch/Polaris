/// win 侧 wire 契约（序列化本体已上提 helper-proto，见 `Response::to_wire_line`；
/// 本测试保留为 win 视角的回归断言 —— win 的 uid 恒 0，`helper-win/helper.go:179`）。
#[test]
fn wire_ok_matches_go_format() {
    use polaris_helper_proto::{Pong, Response, ResponseKind};
    let wire = |k| Response::Ok(k).to_wire_line();
    assert_eq!(
        wire(ResponseKind::Pong(Pong {
            uid: 0,
            proto_version: 5,
            build_identity: None,
        })),
        "OK pong uid=0 v5"
    );
    assert_eq!(wire(ResponseKind::Version { proto_version: 5 }), "OK 5");
    assert_eq!(wire(ResponseKind::Cleaned), "OK cleaned");
    assert_eq!(wire(ResponseKind::Uninstalling), "OK uninstalling");
}

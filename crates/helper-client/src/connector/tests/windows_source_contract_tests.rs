//! Inert source contracts only. Native overlapped cancellation and backpressure
//! require Windows acceptance; these assertions do not claim kernel execution.
#[test]
fn windows_client_owns_overlapped_io_until_completion() {
    let source = polaris_source_probe::crate_source!("windows_pipe.rs");
    let code = polaris_source_probe::mask_comments(&source);
    for anchor in [
        "FILE_FLAG_OVERLAPPED",
        "CreateEventW(",
        "ReadFile(",
        "WriteFile(",
        "WaitForSingleObject(",
        "CancelIoEx(",
        "GetOverlappedResult(",
        "OwnedHandle",
        "fn write_all_until",
    ] {
        assert!(code.contains(anchor), "missing {anchor}");
    }
    assert!(!code.contains("thread::spawn"));
    assert!(code.contains("GetNamedPipeInfo("));
    assert!(code.contains("flags & PIPE_TYPE_MESSAGE != 0"));
    let cancellation = code.split("if wait != WAIT_OBJECT_0").nth(1).unwrap();
    assert!(
        cancellation.find("CancelIoEx(").unwrap()
            < cancellation.find("GetOverlappedResult(").unwrap()
    );
    assert!(
        cancellation.find("GetOverlappedResult(").unwrap()
            < cancellation.find("return Err(error)").unwrap()
    );
}

#[test]
fn windows_read_and_write_use_one_bounded_transfer_owner() {
    let raw = polaris_source_probe::crate_source!("windows_pipe.rs");
    let code = polaris_source_probe::mask_comments(&raw);
    assert!(code.contains("Instant::now() >= deadline"));
    assert!(code.contains("MAX_FRAME_BYTES") && code.contains(".saturating_sub(total)"));
    assert!(code.contains("written != data.len()"));
    assert!(code.contains("ErrorKind::WriteZero"));
    let read = code
        .split("fn read_until_timeout")
        .nth(1)
        .unwrap()
        .split("fn set_read_timeout")
        .next()
        .unwrap();
    let write = code
        .split("fn write_all_until")
        .nth(1)
        .unwrap()
        .split("fn shutdown")
        .next()
        .unwrap();
    assert!(read.contains("self.transfer("));
    assert!(write.contains("self.transfer("));
    assert!(read.contains("is_peer_gone("));
    assert!(!write.contains("is_peer_gone("));
}

#[test]
fn production_connectors_override_absolute_deadline() {
    let raw = polaris_source_probe::crate_source!("connector.rs");
    let code = polaris_source_probe::mask_comments(&raw);
    assert_eq!(code.matches("fn connect_until").count(), 3);
    assert!(code.contains("socket.connect_timeout(&address, remaining)"));
    assert!(!code.contains("UnixStream::connect("));
    assert!(!code.contains("thread::spawn"));
}

// Inert authority-boundary regressions, including the Mac source when this
// harness is compiled elsewhere. These never authenticate a native Root Helper.
fn compact_code(raw: &str) -> String {
    polaris_source_probe::mask_comments_and_strings(raw)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn mac_system_route_is_closed(connector: &str, check: &str, registry: &str, app: &str) -> bool {
    let c = compact_code(connector);
    let h = compact_code(check);
    let r = compact_code(registry);
    let a = compact_code(app);
    let Some(root) = c.split("implConnectorforMacRootConnector{").nth(1) else {
        return false;
    };
    let root = root.split("structUnixConnStream{").next().unwrap();
    let Some(auth) = root.find("nix::unistd::getpeereid(&connection.stream)") else {
        return false;
    };
    let Some(ready) = root.find("Ok(Box::new(connection))") else {
        return false;
    };
    let Some(client) = h.split("implSystemMacCheckClient{").nth(1) else {
        return false;
    };
    let client = client.split("implHelperClient{").next().unwrap();
    auth < ready
        && root.contains("UnixConnector::new(crate::manager::InstallPaths::mac().socket)")
        && root.contains(".connect_stream_until(deadline)?")
        && root.contains("if!uid.is_root(){returnErr(ClientError::Connect(")
        && !root.contains("write_all")
        && c.contains("pub(crate)structMacRootConnector;")
        && h.contains("pubstructSystemMacCheckClient{client:HelperClient,}")
        && client.matches("pubfn").count() == 2
        && client.contains("pubfnsystem(token:String)->Result<Self,String>")
        && client.contains("Box::new(crate::connector::MacRootConnector)")
        && !client.contains("Box<dyn")
        && !h.contains("From<HelperClient>forSystemMacCheckClient")
        && !h.contains("DerefforSystemMacCheckClient")
        && r.contains("pubasyncfnrun_macos_helper_check(client:polaris_helper_client::check_config::SystemMacCheckClient,")
        && a.contains("letclient=polaris_helper_client::check_config::SystemMacCheckClient::system(read_token(&self.token_path()),)?;")
        && a.contains("ifself.never_connect||self.test_connector.is_some(){returnErr(")
}

#[test]
fn generic_same_key_wire_data_cannot_become_production_check_retirement() {
    let connector = polaris_source_probe::crate_source!("connector.rs");
    let check = polaris_source_probe::crate_source!("check_config.rs");
    let registry = polaris_source_probe::repo_file_in(
        env!("CARGO_MANIFEST_DIR"),
        "crates/core-supervisor/src/config_gate/check_custody/macos.rs",
    );
    let app = polaris_source_probe::repo_file_in(
        env!("CARGO_MANIFEST_DIR"),
        "src-tauri/src/runtime/helper.rs",
    );
    assert!(mac_system_route_is_closed(
        &connector, &check, &registry, &app
    ));
    for (slot, old, new) in [
        (
            0,
            "nix::unistd::getpeereid(&connection.stream)",
            "caller_supplied_peer()",
        ),
        (0, "if !uid.is_root()", "if false"),
        (
            0,
            "crate::manager::InstallPaths::mac().socket",
            "caller_supplied_socket()",
        ),
        (
            1,
            "Box::new(crate::connector::MacRootConnector)",
            "caller_supplied_connector",
        ),
        (
            1,
            "    client: HelperClient,",
            "    pub client: HelperClient,",
        ),
        (
            2,
            "client: polaris_helper_client::check_config::SystemMacCheckClient,",
            "client: polaris_helper_client::HelperClient,",
        ),
        (
            3,
            "polaris_helper_client::check_config::SystemMacCheckClient::system(",
            "self.build_client(",
        ),
    ] {
        let mut faces = [
            connector.clone(),
            check.clone(),
            registry.clone(),
            app.clone(),
        ];
        assert!(
            faces[slot].contains(old),
            "negative control must bind actual source: {old}"
        );
        faces[slot] = faces[slot].replacen(old, new, 1);
        assert!(
            !mac_system_route_is_closed(&faces[0], &faces[1], &faces[2], &faces[3]),
            "caller IO/peer/path/accessor regression was not denied: {old}"
        );
    }
}

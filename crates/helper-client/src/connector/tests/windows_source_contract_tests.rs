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

// Each predicate reads one balanced impl. An adjacent platform's methods
// cannot supply evidence for a missing deadline or a closed client API.
fn balanced_block<'a>(code: &'a str, head: &str) -> Option<&'a str> {
    if code.matches(head).count() != 1 || !head.ends_with('{') {
        return None;
    }
    let start = code.find(head)?;
    let mut depth = 1usize;
    for (offset, byte) in code[start + head.len()..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&code[start..start + head.len() + offset + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

fn connectors_keep_one_absolute_deadline(raw: &str) -> bool {
    let code = compact_code(raw);
    let owners = [
        "UnixConnector",
        "MacRootConnector",
        "PipeConnector",
        "WindowsSystemConnector",
    ];
    if code.matches("implConnectorfor").count() != owners.len()
        || code.matches("fnconnect_until(").count() != owners.len()
        || code.contains("UnixStream::connect(")
        || code.contains("thread::spawn")
        || !code.contains("socket.connect_timeout(&address,remaining)")
    {
        return false;
    }
    for owner in owners {
        let Some(body) = balanced_block(&code, &format!("implConnectorfor{owner}{{")) else {
            return false;
        };
        if body
            .matches("fnconnect_until(&self,deadline:Instant)")
            .count()
            != 1
            || !body.contains("self.connect_until(Instant::now()+")
        {
            return false;
        }
        match owner {
            "UnixConnector" | "MacRootConnector"
                if !body.contains(".connect_stream_until(deadline)?") =>
            {
                return false
            }
            "PipeConnector"
                if !body.contains("WinPipeStream::connect(&self.pipe_path)")
                    || !body.contains("Instant::now()>=deadline")
                    || !body.contains(".saturating_duration_since(Instant::now())") =>
            {
                return false
            }
            "WindowsSystemConnector"
                if !body.contains("crate::windows_pipe::WinPipeStream::connect_system()")
                    || !body.contains("Instant::now()>=deadline")
                    || !body.contains(
                        "Ok(stream)ifInstant::now()<deadline=>returnOk(Box::new(stream))",
                    )
                    || !body.contains("error.raw_os_error()==Some(231)")
                    || !body.contains(".saturating_duration_since(Instant::now())") =>
            {
                return false
            }
            _ => {}
        }
    }
    true
}

#[test]
fn production_connectors_override_absolute_deadline() {
    let raw = polaris_source_probe::crate_source!("connector.rs");
    assert!(connectors_keep_one_absolute_deadline(&raw));
    for (old, new) in [
        (
            "impl Connector for WindowsSystemConnector",
            "impl Connector for UnknownConnector",
        ),
        (
            "WinPipeStream::connect_system()",
            "WinPipeStream::connect(caller_path)",
        ),
        (
            ".connect_stream_until(deadline)?",
            ".connect_stream_until(Instant::now() + READ_TIMEOUT)?",
        ),
        (
            "Ok(stream) if Instant::now() < deadline",
            "Ok(stream) if true",
        ),
    ] {
        assert!(
            raw.contains(old),
            "negative must mutate actual deadline owner: {old}"
        );
        assert!(!connectors_keep_one_absolute_deadline(
            &raw.replacen(old, new, 1)
        ));
    }
    assert!(!connectors_keep_one_absolute_deadline(&format!("{raw}\nimpl Connector for NewConnector {{ fn connect_until(&self, deadline: Instant) {{}} }}")));
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
    let Some(app_check) = balanced_block(&a, "pub(crate)asyncfncheck_config(&self,config:&Path,)->Result<polaris_core_supervisor::RawCheck,String>{") else {
        return false;
    };
    let Some(root) = balanced_block(&c, "implConnectorforMacRootConnector{") else {
        return false;
    };
    let Some(windows) = balanced_block(&c, "implConnectorforWindowsSystemConnector{") else {
        return false;
    };
    let Some(auth) = root.find("nix::unistd::getpeereid(&connection.stream)") else {
        return false;
    };
    let Some(ready) = root.find("Ok(Box::new(connection))") else {
        return false;
    };
    for (owner, issuer, platform, operation, digest) in [
        (
            "SystemMacCheckClient",
            "MacRootConnector",
            "Mac",
            "run_macos_check",
            "sha256",
        ),
        (
            "SystemWindowsCheckClient",
            "WindowsSystemConnector",
            "Win",
            "run_windows_check",
            "digest",
        ),
    ] {
        let Some(client) = balanced_block(&h, &format!("impl{owner}{{")) else {
            return false;
        };
        if !h.contains(&format!("pubstruct{owner}{{client:HelperClient,}}"))
            || client.matches("pub").count() != 4
            || !client.contains("pubfnsystem(token:String)->Result<Self,String>{")
            || !client.contains("pubfnnative_birth_status(&self,)->Result<polaris_helper_proto::response::NativeBirthStatus,String>{")
            || !client.contains("pubfnrun_check(&self,core:CoreId,config:&[u8],digest:[u8;32],cancel:&AtomicBool,)->Result<HelperCheckOutput,HelperCheckFailure>{")
            || !client.contains(&format!("pubfn{operation}(&self,core:CoreId,config:&[u8],{digest}:[u8;32],cancel:&AtomicBool,)->Result<HelperCheckOutput,HelperCheckFailure>{{"))
            || !client.contains(&format!("Box::new(crate::connector::{issuer}),Platform::{platform},token,"))
            || client.contains("Box<dyn")
            || h.contains(&format!("From<HelperClient>for{owner}"))
            || h.contains(&format!("Dereffor{owner}"))
        { return false; }
    }
    let aliases: String = polaris_source_probe::mask_comments(registry)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    auth < ready
        && root.contains("UnixConnector::new(crate::manager::InstallPaths::mac().socket)")
        && root.contains(".connect_stream_until(deadline)?")
        && root.contains("if!uid.is_root(){returnErr(ClientError::Connect(")
        && !root.contains("write_all")
        && c.contains("pub(crate)structMacRootConnector;")
        && c.contains("pub(crate)structWindowsSystemConnector;")
        && windows.contains("crate::windows_pipe::WinPipeStream::connect_system()")
        && !windows.contains("PipeConnector")
        && r.matches("typeSystemCheckClient=").count() == 2
        && aliases.contains("#[cfg(target_os=\"macos\")]typeSystemCheckClient=polaris_helper_client::check_config::SystemMacCheckClient;")
        && aliases.contains("#[cfg(windows)]typeSystemCheckClient=polaris_helper_client::check_config::SystemWindowsCheckClient;")
        && r.contains("pubasyncfnrun_macos_helper_check(client:SystemCheckClient,")
        && app_check.contains("letclient=polaris_helper_client::check_config::SystemMacCheckClient::system(read_token(&self.token_path()),)?;")
        && app_check.contains("letclient=polaris_helper_client::check_config::SystemWindowsCheckClient::system(read_token(&self.token_path()),)?;")
        && app_check.contains("ifself.never_connect||self.test_connector.is_some(){returnErr(")
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
            "client: SystemCheckClient,",
            "client: polaris_helper_client::HelperClient,",
        ),
        (
            3,
            "polaris_helper_client::check_config::SystemMacCheckClient::system(",
            "self.build_client(",
        ),
        (1, "impl SystemMacCheckClient {", "impl UntrustedClient {"),
        (1, "    pub fn run_macos_check(", "    pub fn caller_transport("),
        (1, "    pub fn native_birth_status(", "    fn native_birth_status("),
        (1, "impl SystemWindowsCheckClient {", "impl SystemWindowsCheckClient {\n    pub fn from_client(client: HelperClient) -> Self { Self { client } }"),
        (1, "Box::new(crate::connector::WindowsSystemConnector)", "caller_supplied_connector"),
        (2, "#[cfg(windows)]\ntype SystemCheckClient", "#[cfg(unix)]\ntype SystemCheckClient"),
        (2, "type SystemCheckClient = polaris_helper_client::check_config::SystemWindowsCheckClient;", "type SystemCheckClient = polaris_helper_client::check_config::SystemMacCheckClient;"),
        (3, "polaris_helper_client::check_config::SystemWindowsCheckClient::system(", "self.build_client("),
        (3, "if self.never_connect || self.test_connector.is_some()", "if false"),
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
        if slot == 3 {
            assert!(
                faces[slot].contains(old),
                "the neighboring status query must remain as a decoy: {old}"
            );
        }
        assert!(
            !mac_system_route_is_closed(&faces[0], &faces[1], &faces[2], &faces[3]),
            "caller IO/peer/path/accessor regression was not denied: {old}"
        );
    }
}

use super::*;

fn expected() -> Expected<'static> {
    Expected {
        label: "com.example.helper",
        plist_path: Path::new("/Library/LaunchDaemons/com.example.helper.plist"),
        helper_path: Path::new("/Library/PrivilegedHelperTools/com.example.helper"),
        support_path: Path::new("/Library/Application Support/Example"),
        core_dir: Path::new("/Library/Application Support/Example/core"),
    }
}

fn modern() -> Vec<String> {
    let e = expected();
    vec![
        e.helper_path.display().to_string(),
        "--support".into(),
        e.support_path.display().to_string(),
        "--coredir".into(),
        e.core_dir.display().to_string(),
    ]
}

fn legacy() -> Vec<String> {
    let mut args = modern();
    args.extend([
        "--singbox".into(),
        expected().core_dir.join("sing-box").display().to_string(),
        "--confdir".into(),
        "/Users/example/Library/Application Support/com.example.app/config".into(),
    ]);
    args
}

fn dictionary(args: Vec<String>) -> plist::Dictionary {
    let mut dict = plist::Dictionary::new();
    dict.insert(
        "Label".into(),
        plist::Value::String(expected().label.into()),
    );
    dict.insert(
        "ProgramArguments".into(),
        plist::Value::Array(args.into_iter().map(plist::Value::String).collect()),
    );
    dict.insert("KeepAlive".into(), plist::Value::Boolean(true));
    dict.insert("RunAtLoad".into(), plist::Value::Boolean(true));
    dict
}

fn xml(dict: plist::Dictionary) -> Vec<u8> {
    let mut bytes = Vec::new();
    plist::Value::Dictionary(dict)
        .to_writer_xml(&mut bytes)
        .unwrap();
    bytes
}

fn facts() -> PlistFacts {
    parse_plist(&xml(dictionary(legacy())), &expected()).unwrap()
}

fn loaded(facts: &PlistFacts) -> String {
    let args: String = facts
        .arguments
        .iter()
        .map(|a| format!("\t\t{a}\n"))
        .collect();
    let props: Vec<_> = [
        (facts.keep_alive, "keepalive"),
        (facts.run_at_load, "runatload"),
        (facts.inferred_program, "inferred program"),
    ]
    .into_iter()
    .filter_map(|(enabled, name)| enabled.then_some(name))
    .collect();
    format!(
        "system/{} = {{\n\tpath = {}\n\ttype = LaunchDaemon\n\tstate = running\n\tprogram = {}\n\targuments = {{\n{}\t}}\n\tdefault environment = {{\n\t\tPATH => /usr/bin:/bin\n\t}}\n\tenvironment = {{\n\t\tOSLogRateLimit => 64\n\t\tXPC_SERVICE_NAME => {}\n\t}}\n\tdomain = system\n\texit timeout = 5\n\tpid = 321\n\tstarted suspended = 0\n\tproxy started suspended = 0\n\tresource coalition = {{\n\t\tID = 789\n\t\ttype = resource\n\t\tstate = active\n\t\tactive count = 1\n\t\tname = {}\n\t}}\n\tspawn type = daemon (3)\n\tjob state = running\n\tsanitizer flags = 0x0\n\tproperties = {}\n}}\n",
        facts.label,
        expected().plist_path.display(),
        facts.program.display(),
        args,
        facts.label,
        facts.label,
        props.join(" | ")
    )
}

#[test]
fn legacy_and_root_multiuser_candidate_are_supported() {
    let old = facts();
    assert!(old.singbox.is_some() && old.confdir.is_some());
    let mut args = modern();
    args.push("--upgrade-closed".into());
    let new = parse_plist(&xml(dictionary(args)), &expected()).unwrap();
    assert!(new.singbox.is_none() && new.confdir.is_none() && new.upgrade_closed);
    validate_loaded_print(&loaded(&new), &new, &expected()).unwrap();
}

#[test]
fn content_addressed_singbox_is_accepted_as_a_path_fact() {
    let mut args = legacy();
    args[6] = format!(
        "{}/versions/{}/sing-box",
        expected().core_dir.display(),
        "a".repeat(64)
    );
    assert!(parse_plist(&xml(dictionary(args)), &expected()).is_ok());
}

#[test]
fn binary_plist_uses_the_same_schema() {
    let mut bytes = Vec::new();
    plist::Value::Dictionary(dictionary(modern()))
        .to_writer_binary(&mut bytes)
        .unwrap();
    assert!(parse_plist(&bytes, &expected()).is_ok());
}

#[test]
fn explicit_program_must_match_and_is_not_inferred() {
    let mut dict = dictionary(modern());
    dict.insert("Program".into(), plist::Value::String(modern()[0].clone()));
    let parsed = parse_plist(&xml(dict.clone()), &expected()).unwrap();
    assert!(!parsed.inferred_program);
    validate_loaded_print(&loaded(&parsed), &parsed, &expected()).unwrap();
    dict.insert("Program".into(), plist::Value::String("/bin/other".into()));
    assert!(parse_plist(&xml(dict), &expected()).is_err());
}

#[test]
fn unknown_or_identity_affecting_plist_keys_are_not_ignored() {
    for name in [
        "UserName",
        "GroupName",
        "AbandonProcessGroup",
        "EnableGlobbing",
        "EnvironmentVariables",
        "RootDirectory",
        "WorkingDirectory",
        "WaitForDebugger",
        "Debug",
        "MachServices",
        "Sockets",
        "StandardOutPath",
        "Disabled",
        "NewFutureKey",
    ] {
        let mut dict = dictionary(modern());
        dict.insert(name.into(), plist::Value::Boolean(false));
        assert!(parse_plist(&xml(dict), &expected()).is_err(), "{name}");
    }
}

#[test]
fn duplicate_xml_fields_reject_before_dictionary_collapse() {
    let bytes = xml(dictionary(modern()));
    let text = String::from_utf8(bytes).unwrap();
    for duplicate in [
        "<key>Label</key><string>com.example.helper</string>",
        "<key>KeepAlive</key><true/>",
        "<key>ProgramArguments</key><array/>",
        "<key>Program</key><string>/bin/other</string><key>Program</key><string>/bin/other</string>",
    ] {
        let changed = text.replacen("</dict>", &format!("{duplicate}</dict>"), 1);
        assert!(parse_plist(changed.as_bytes(), &expected()).is_err());
    }
}

#[test]
fn wrong_types_missing_keys_and_truncation_reject() {
    for name in ["Label", "ProgramArguments", "KeepAlive", "RunAtLoad"] {
        let mut dict = dictionary(modern());
        dict.remove(name);
        assert!(parse_plist(&xml(dict), &expected()).is_err());
    }
    let mut dict = dictionary(modern());
    dict.insert(
        "KeepAlive".into(),
        plist::Value::Dictionary(plist::Dictionary::new()),
    );
    assert!(parse_plist(&xml(dict), &expected()).is_err());
    let bytes = xml(dictionary(modern()));
    assert!(parse_plist(&bytes[..bytes.len() / 2], &expected()).is_err());
    assert!(parse_plist(&vec![0; MAX_PLIST + 1], &expected()).is_err());
}

#[test]
fn argv_requires_exact_unique_supported_flags() {
    for extra in [
        vec!["--unknown"],
        vec!["--support"],
        vec!["--support", "/elsewhere"],
        vec!["--upgrade-closed", "--upgrade-closed"],
        vec!["--upgrade-closed=true"],
        vec!["--upgrade-closed", "true"],
        vec!["operand"],
    ] {
        let mut args = modern();
        args.extend(extra.iter().map(|s| s.to_string()));
        assert!(parse_plist(&xml(dictionary(args)), &expected()).is_err());
    }
    let mut args = modern();
    args.truncate(3);
    assert!(parse_plist(&xml(dictionary(args)), &expected()).is_err());
}

#[test]
fn ambiguous_non_absolute_and_unmanaged_paths_reject() {
    for bad in [
        "relative", "/a/../b", "/a/./b", "//a", "/a/", "/a\nb", "/a\tb", " /a", "/a ",
    ] {
        let mut args = modern();
        args.extend(["--confdir".into(), bad.into()]);
        assert!(
            parse_plist(&xml(dictionary(args)), &expected()).is_err(),
            "{bad:?}"
        );
    }
    for bad in [
        "/bin/sing-box",
        "/Library/Application Support/Example/core/versions/bad/sing-box",
    ] {
        let mut args = legacy();
        args[6] = bad.into();
        assert!(parse_plist(&xml(dictionary(args)), &expected()).is_err());
    }
}

#[test]
fn actual_loaded_timeout_is_not_inferred_from_missing_plist_key() {
    let facts = facts();
    assert_eq!(facts.exit_timeout_seconds, None);
    assert_eq!(
        validate_loaded_print(&loaded(&facts), &facts, &expected())
            .unwrap()
            .exit_timeout_seconds,
        5
    );
}

#[test]
fn explicit_timeout_must_match_and_zero_is_preserved() {
    let mut dict = dictionary(modern());
    dict.insert("ExitTimeOut".into(), plist::Value::Integer(0.into()));
    let facts = parse_plist(&xml(dict), &expected()).unwrap();
    assert!(validate_loaded_print(&loaded(&facts), &facts, &expected()).is_err());
    let text = loaded(&facts).replace("exit timeout = 5", "exit timeout = 0");
    assert_eq!(
        validate_loaded_print(&text, &facts, &expected())
            .unwrap()
            .exit_timeout_seconds,
        0
    );
}

#[test]
fn negative_or_noninteger_timeout_rejects() {
    for value in [
        plist::Value::Integer((-1).into()),
        plist::Value::String("5".into()),
    ] {
        let mut dict = dictionary(modern());
        dict.insert("ExitTimeOut".into(), value);
        assert!(parse_plist(&xml(dict), &expected()).is_err());
    }
}

#[test]
fn nested_state_cannot_satisfy_required_top_level_state() {
    let facts = facts();
    let text = loaded(&facts).replacen("\tstate = running\n", "", 1);
    assert!(validate_loaded_print(&text, &facts, &expected()).is_err());
}

#[test]
fn duplicate_fields_or_blocks_and_nested_identity_are_rejected() {
    let facts = facts();
    for insertion in ["\tpid = 321\n", "\targuments = {\n\t}\n", "\tunknown = 1\n"] {
        let text = loaded(&facts).replacen("\tpath = ", &format!("{insertion}\tpath = "), 1);
        assert!(validate_loaded_print(&text, &facts, &expected()).is_err());
    }
    let text = loaded(&facts).replace("\t\tID = 789", "\t\tpid = 321");
    assert!(validate_loaded_print(&text, &facts, &expected()).is_err());
}

#[test]
fn loaded_mismatched_authoritative_fields_reject() {
    let facts = facts();
    for (old, new) in [
        ("system/com.example.helper", "gui/com.example.helper"),
        ("type = LaunchDaemon", "type = LaunchAgent"),
        ("state = running", "state = waiting"),
        ("domain = system", "domain = gui"),
        ("pid = 321", "pid = 0"),
        ("pid = 321", "pid = 0321"),
        ("pid = 321", "pid = 2147483648"),
        ("exit timeout = 5", "exit timeout = -1"),
        ("started suspended = 0", "started suspended = 1"),
        ("sanitizer flags = 0x0", "sanitizer flags = 0x1"),
    ] {
        assert!(
            validate_loaded_print(&loaded(&facts).replace(old, new), &facts, &expected()).is_err()
        );
    }
}

#[test]
fn extra_runtime_environment_or_properties_reject() {
    let facts = facts();
    for (old, new) in [
        ("OSLogRateLimit", "DYLD_INSERT_LIBRARIES"),
        (
            "keepalive | runatload",
            "keepalive | runatload | abandon process group",
        ),
        ("keepalive | runatload", "keepalive | keepalive | runatload"),
    ] {
        assert!(
            validate_loaded_print(&loaded(&facts).replace(old, new), &facts, &expected()).is_err()
        );
    }
}

#[test]
fn incomplete_extra_root_and_multiline_argument_reject() {
    let facts = facts();
    let good = loaded(&facts);
    for text in [
        good[..good.len() - 2].to_owned(),
        format!("{good}}}\n"),
        good.replace("\t\t--support\n", "\t\t--support\n\t\textra\n"),
        good.replace('\n', "\r\n"),
    ] {
        assert!(validate_loaded_print(&text, &facts, &expected()).is_err());
    }
}

#[test]
fn disabled_requires_exact_unique_explicit_target_boolean() {
    let label = expected().label;
    let yes = format!("disabled services = {{\n\t\"{label}\" => true\n}}\n");
    assert!(parse_disabled_print(&yes, label).unwrap());
    assert!(!parse_disabled_print(&yes.replace("true", "false"), label).unwrap());
    for bad in [
        "disabled services = {\n}\n".to_owned(),
        yes.replace(label, "com.example.helper.other"),
        yes.replace("true", "1"),
        yes.replace("\t\"", "\t\t\""),
        yes.replace("}\n", &format!("\t\"{label}\" => true\n}}\n")),
        yes.trim_end().to_owned(),
    ] {
        assert!(parse_disabled_print(&bad, label).is_err());
    }
}

fn domain(rows: &str) -> String {
    format!("system = {{\n\ttype = system\n\tservices = {{\n{rows}\t}}\n\tother = {{\n\t\tstate = active\n\t}}\n}}\n")
}

#[test]
fn services_absence_is_positive_complete_table_evidence() {
    let label = expected().label;
    let text = domain("\t\t       0   (pe) \tcom.example.other\n\t\t      42      - \tcom.example.running\n\t\t       0     -1 \tcom.example.failed\n");
    assert!(parse_system_services_absent(&text, label).unwrap());
    for pid in [0, 321] {
        assert!(
            !parse_system_services_absent(&domain(&format!("\t\t{pid} - {label}\n")), label)
                .unwrap()
        );
    }
}

#[test]
fn services_nested_table_is_not_authoritative() {
    let text = "system = {\n\tother = {\n\t\tservices = {\n\t\t}\n\t}\n}\n";
    assert!(parse_system_services_absent(text, expected().label).is_err());
}

#[test]
fn services_duplicates_unparsed_rows_and_truncation_reject() {
    let label = expected().label;
    for text in [
        domain("\t\t0 - com.example.other\n\t\t0 - com.example.other\n"),
        domain("\t\t0 - com.example.other extra\n"),
        domain("\t\t0 ??? com.example.other\n"),
        domain("\t\t0 - com.example.other\n").replace("\tother = {", "\tservices = {"),
        domain("").trim_end_matches("}\n").to_owned(),
        "system = {\n\tservices = {\n}\n".into(),
        "system = {\n\tservices = {}\n}\n".into(),
        domain("").replace("system = {", "gui = {"),
    ] {
        assert!(
            parse_system_services_absent(&text, label).is_err(),
            "{text}"
        );
    }
}

#[test]
fn sanitized_full_observed_shape_is_supported() {
    let old = legacy();
    let args = [0, 5, 6, 7, 8, 1, 2, 3, 4]
        .into_iter()
        .map(|i| old[i].clone())
        .collect();
    let facts = parse_plist(&xml(dictionary(args)), &expected()).unwrap();
    let observed =
        validate_loaded_print(include_str!("full-job-print.txt"), &facts, &expected()).unwrap();
    assert_eq!(observed.pid, 321);
    assert_eq!(observed.exit_timeout_seconds, 5);
}

#[test]
fn missing_disabled_state_is_distinct_from_explicit_false_and_true() {
    let label = expected().label;
    assert_eq!(
        parse_disabled_state("disabled services = {\n}\n", label).unwrap(),
        None
    );
    for flag in [false, true] {
        let text = format!("disabled services = {{\n\t\"{label}\" => {flag}\n}}\n");
        assert_eq!(parse_disabled_state(&text, label).unwrap(), Some(flag));
    }
    assert!(parse_disabled_state("disabled services = {\n", label).is_err());
}

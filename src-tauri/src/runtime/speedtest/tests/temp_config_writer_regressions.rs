use super::super::*;

#[test]
fn private_writer_obeys_store_suffix_contract_on_create_and_replace() {
    let dir = std::env::temp_dir().join(format!(
        "polaris-speedtest-writer-contract-{}",
        polaris_store::fs::random_tmp_suffix(),
    ));
    std::fs::create_dir(&dir).unwrap();
    let config = dir.join(TEMP_CORE_CONFIG_NAME);
    // Run the actual production helper and durable writer, including tmp_path's
    // debug assertion. A made-up PID/counter suffix fails here in debug tests.
    for value in [
        b"first".as_slice(),
        b"replacement".as_slice(),
        b"final".as_slice(),
    ] {
        write_private_temp_config(&config, value).unwrap();
        assert_eq!(std::fs::read(&config).unwrap(), value);
        let entries = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            vec![std::ffi::OsString::from(TEMP_CORE_CONFIG_NAME)],
            "each successful publication must consume its exclusive stage"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

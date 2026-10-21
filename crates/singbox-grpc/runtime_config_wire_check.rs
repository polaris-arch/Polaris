// Optional source-contract check only. This does not certify any built kernel.
// Do not put this additive API in StartedService's mandatory released-core table.
#[allow(dead_code)]
mod runtime_config_wire_check {
    pub const SOURCE: &[u8] = include_bytes!("proto/runtime_config.proto");
    pub const MAX_SOURCE_BYTES: u64 = 64 * 1024;

    pub fn compare_source(source: &[u8]) -> Result<(), String> {
        if source.len() as u64 > MAX_SOURCE_BYTES {
            return Err("runtime config proto source exceeds bounded contract check".into());
        }
        if source != SOURCE {
            return Err("runtime config proto differs from the exact authored kernel source".into());
        }
        Ok(())
    }

    pub fn check_optional_source() -> Result<(), Box<dyn std::error::Error>> {
        println!("cargo:rerun-if-env-changed=POLARIS_RUNTIME_CONFIG_PROTO_SOURCE");
        if let Some(path) = std::env::var_os("POLARIS_RUNTIME_CONFIG_PROTO_SOURCE") {
            use std::io::Read;
            let path = std::path::PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            let file = std::fs::File::open(&path)?;
            let mut bytes = Vec::new();
            file.take(MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes)?;
            compare_source(&bytes)?;
        }
        Ok(())
    }
}

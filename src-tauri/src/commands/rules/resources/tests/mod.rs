// SRS magic, version, and a zlib stream containing a zero rule count.
const EMPTY_SRS_V1: &[u8] = b"SRS\x01\x78\x9c\x63\x00\x00\x00\x01\x00\x01";
const EMPTY_SRS_V2: &[u8] = b"SRS\x02\x78\x9c\x63\x00\x00\x00\x01\x00\x01";

mod builtin_update_tests;
mod gh_proxy_tests;

mod resource_delete_tests;

mod resource_download_tests;

mod catalog_tests;

mod manual_update_tests;

mod download_identity_regressions;

use super::*;
#[test]
fn malformed_and_non_pe_inputs_never_prove_a_loader() {
    for b in [
        vec![],
        b"ordinary data".to_vec(),
        b"MZ".to_vec(),
        vec![0; 4096],
    ] {
        assert!(imports(&b).is_err());
    }
}

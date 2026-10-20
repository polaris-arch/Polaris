use super::super::*;

#[test]
fn dependency_response_preserves_code_without_user_identifiers() {
    let response: ApiResponse<()> = config_store_error(polaris_store::StoreError::DnsDependency(
        "https://user:secret@example.invalid/private".to_owned(),
    ));
    assert!(!response.success);
    assert_eq!(response.code.as_deref(), Some("DNS_DEPENDENCY_INVALID"));
    assert_eq!(
        response.error.as_deref(),
        Some("DNS dependency is invalid or still in use")
    );
}

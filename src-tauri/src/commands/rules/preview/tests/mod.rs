use super::preview;
#[test]
fn ascii_subset_is_case_sensitive_and_unsupported_is_unknown() {
    let p = [
        "^Example[.]com$",
        "^example[.]com$",
        "(?i)example",
        r"\p{L}+",
        "(?=a)a",
        "[a&&b]",
    ];
    assert_eq!(
        preview(&p.map(str::to_owned), "Example.com").unwrap(),
        vec![Some(true), Some(false), None, None, None, None]
    );
}
#[test]
fn bounded_without_truncation_or_backtracking() {
    assert!(preview(&vec!["a".into(); 33], "a").is_err());
    assert!(preview(&["a".repeat(1025)], "a").is_err());
    assert!(preview(&[], &"a".repeat(4097)).is_err());
    assert_eq!(
        preview(&["(a+)+$".into()], "aaaaaaaa!").unwrap(),
        vec![Some(false)]
    );
    assert_eq!(preview(&["[".into()], "a").unwrap(), vec![None]);
}

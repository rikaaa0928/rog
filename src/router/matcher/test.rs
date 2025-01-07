use crate::router::matcher::{get_matcher_factory_fn, Matcher};

#[test]
fn test_cidr_matcher() {
    let lines = vec!["192.168.1.0/24".to_string(), "10.0.0.1/32".to_string()];
    let factory = get_matcher_factory_fn("cicd").unwrap();
    let matcher = factory(lines, vec![]);
    assert!(matcher.match_host("192.168.1.1"));
    assert!(matcher.match_host("10.0.0.1"));
    assert!(!matcher.match_host("192.168.2.1"));
}

#[test]
fn test_regex_matcher() {
    let lines = vec!["^www\\.example\\.com$".to_string(), "^test\\.example\\.com$".to_string()];
    let factory = get_matcher_factory_fn("regex").unwrap();
    let matcher = factory(lines, vec![]);
    assert!(matcher.match_host("www.example.com"));
    assert!(matcher.match_host("test.example.com"));
    assert!(!matcher.match_host("sub.example.com"));
}

#[test]
fn test_get_matcher_factory_fn_invalid() {
    assert!(get_matcher_factory_fn("invalid").is_none());
}

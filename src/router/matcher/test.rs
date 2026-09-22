#[cfg(test)]
mod tests {
    use crate::router::consts;
    use crate::router::matcher::get_matcher_factory_fn;

    #[test]
    fn test_cidr_matcher() {
        let lines = vec!["192.168.1.0/24".to_string(), "10.0.0.1/32".to_string()];
        let factory = get_matcher_factory_fn(consts::FORMAT_CIDR).unwrap();
        let matcher = factory(lines, vec![]);
        assert!(matcher.match_host("192.168.1.1"));
        assert!(matcher.match_host("10.0.0.1"));
        assert!(!matcher.match_host("192.168.2.1"));
    }

    #[test]
    fn test_regex_matcher() {
        let lines = vec![
            "^www\\.example\\.com$".to_string(),
            "^test\\.example\\.com$".to_string(),
        ];
        let factory = get_matcher_factory_fn("regex").unwrap();
        let matcher = factory(lines, vec![]);
        assert!(matcher.match_host("www.example.com"));
        assert!(matcher.match_host("test.example.com"));
        assert!(!matcher.match_host("sub.example.com"));
    }

    #[test]
    fn test_domain_suffix_matcher() {
        let lines = vec![
            "google.com".to_string(),
            ".baidu.com".to_string(),
            "# comment line".to_string(),
            "".to_string(),
        ];
        for format in &["domain-suffix", "domain_suffix", "suffix"] {
            let factory = get_matcher_factory_fn(format).unwrap();
            let matcher = factory(lines.clone(), vec![]);
            assert!(matcher.match_host("google.com"));
            assert!(matcher.match_host("www.google.com"));
            assert!(matcher.match_host("sub.mail.google.com"));
            assert!(matcher.match_host("Google.COM"));
            assert!(matcher.match_host("baidu.com"));
            assert!(matcher.match_host("news.baidu.com"));

            // Non matches
            assert!(!matcher.match_host("badgoogle.com"));
            assert!(!matcher.match_host("google.com.cn"));
            assert!(!matcher.match_host("gle.com"));
            assert!(!matcher.match_host("qq.com"));

            // IP addresses shouldn't match domain suffix
            assert!(!matcher.match_host("8.8.8.8"));
            assert!(!matcher.match_host("127.0.0.1"));
        }
    }

    #[test]
    fn test_domain_keyword_matcher() {
        let lines = vec![
            "google".to_string(),
            "twitter".to_string(),
            "# comment".to_string(),
        ];
        for format in &["domain-keyword", "domain_keyword", "keyword"] {
            let factory = get_matcher_factory_fn(format).unwrap();
            let matcher = factory(lines.clone(), vec![]);
            assert!(matcher.match_host("google.com"));
            assert!(matcher.match_host("www.google.com"));
            assert!(matcher.match_host("my-google-app.com"));
            assert!(matcher.match_host("Google.COM"));
            assert!(matcher.match_host("api.twitter.com"));

            // Non matches
            assert!(!matcher.match_host("baidu.com"));
            assert!(!matcher.match_host("github.com"));

            // IP addresses shouldn't match domain keyword
            assert!(!matcher.match_host("8.8.8.8"));
        }
    }

    #[test]
    fn test_get_matcher_factory_fn_invalid() {
        assert!(get_matcher_factory_fn("invalid").is_none());
    }
}

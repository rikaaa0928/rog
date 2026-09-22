#[cfg(test)]
mod tests {
    use crate::router::resolver::Resolver;
    use std::time::Duration;

    #[tokio::test]
    async fn test_resolve_ip_with_specific_dns() {
        let resolver = Resolver::new();
        let dns_config = "8.8.8.8:53";
        let result = resolver
            .resolve_ip_with_specific_dns("www.google.com", dns_config)
            .await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_resolve_ip_with_default_dns() {
        let resolver = Resolver::new();
        let result = resolver
            .resolve_ip_with_default_dns("www.example.com")
            .await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_resolve_ip_with_doh() {
        let resolver = Resolver::new();
        let dns_config = "doh://cloudflare-dns.com/dns-query";
        let result = resolver
            .resolve_ip_with_doh("www.example.com", dns_config)
            .await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_cache_expiration() {
        let resolver = Resolver::new();
        let dns_config = "8.8.8.8:53";
        resolver
            .resolve_ip("www.baidu.com", dns_config)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(resolver.cache.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_negative_cache_expiration() {
        let resolver = Resolver::new();
        let dns_config = "invalid-dns-address";
        resolver
            .resolve_ip("www.example.com", dns_config)
            .await
            .unwrap_err();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(resolver.cache.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_router_with_domain_suffix_and_keyword() {
        use crate::def::RouterSet;
        use crate::def::config::{RouteData, RouteRule, Router};
        use crate::router::DefaultRouter;
        use crate::util::RunAddr;

        let resolver = Resolver::new();
        let data_cfg = vec![
            RouteData {
                name: "suffix_rule".to_string(),
                url: None,
                format: "domain-suffix".to_string(),
                data: Some("google.com\nbaidu.com".to_string()),
                interval: None,
            },
            RouteData {
                name: "keyword_rule".to_string(),
                url: None,
                format: "domain-keyword".to_string(),
                data: Some("twitter\ngithub".to_string()),
                interval: None,
            },
        ];
        let router_cfg = vec![Router {
            name: "test_router".to_string(),
            default: "direct".to_string(),
            route_rules: Some(vec![
                RouteRule {
                    name: "suffix_rule".to_string(),
                    select: "proxy_suffix".to_string(),
                    exclude: vec![],
                    domain_to_ip: None,
                    dns: None,
                },
                RouteRule {
                    name: "keyword_rule".to_string(),
                    select: "proxy_keyword".to_string(),
                    exclude: vec![],
                    domain_to_ip: None,
                    dns: None,
                },
            ]),
        }];

        let router = DefaultRouter::new(&router_cfg, &data_cfg, resolver).await;

        let addr1 = RunAddr {
            addr: "www.google.com".to_string(),
            port: 443,
            udp: false,
        };
        assert_eq!(
            router.route("l1", "test_router", &addr1).await,
            "proxy_suffix"
        );

        let addr2 = RunAddr {
            addr: "api.twitter.com".to_string(),
            port: 443,
            udp: false,
        };
        assert_eq!(
            router.route("l1", "test_router", &addr2).await,
            "proxy_keyword"
        );

        let addr3 = RunAddr {
            addr: "example.com".to_string(),
            port: 80,
            udp: false,
        };
        assert_eq!(router.route("l1", "test_router", &addr3).await, "direct");
    }

    #[tokio::test]
    async fn test_dynamic_remote_matcher_non_blocking_and_error_handling() {
        use crate::def::RouterSet;
        use crate::def::config::{RouteData, RouteRule, Router};
        use crate::router::DefaultRouter;
        use crate::util::RunAddr;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let request_count = Arc::new(AtomicUsize::new(0));
        let count_clone = request_count.clone();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if let Ok(mut stream) = stream {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf);
                    let count = count_clone.fetch_add(1, Ordering::SeqCst);
                    if count < 4 {
                        // First 4 requests (1 initial + 3 retries) fail with 500 to test retry exhaustion & graceful fallback
                        let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(response.as_bytes());
                    } else {
                        // Subsequent request returns valid domain suffix list
                        let body = "google.com\nbaidu.com\n";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                }
            }
        });

        let resolver = Resolver::new();
        let data_cfg = vec![RouteData {
            name: "remote_suffix".to_string(),
            url: Some(format!("http://127.0.0.1:{}/suffix.txt", port)),
            format: "domain-suffix".to_string(),
            data: None,
            interval: Some(1), // 1 second interval
        }];
        let router_cfg = vec![Router {
            name: "test_router".to_string(),
            default: "direct".to_string(),
            route_rules: Some(vec![RouteRule {
                name: "remote_suffix".to_string(),
                select: "proxy_remote".to_string(),
                exclude: vec![],
                domain_to_ip: None,
                dns: None,
            }]),
        }];

        // Router creation is completely NON-BLOCKING!
        let router = DefaultRouter::new(&router_cfg, &data_cfg, resolver).await;

        let addr = RunAddr {
            addr: "www.google.com".to_string(),
            port: 443,
            udp: false,
        };

        // Immediately route: since initial fetch failed (500), it treats as empty list -> returns default "direct"
        assert_eq!(router.route("l1", "test_router", &addr).await, "direct");

        // Wait for next interval to fetch valid data
        let mut routed = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if router.route("l1", "test_router", &addr).await == "proxy_remote" {
                routed = true;
                break;
            }
        }
        assert!(
            routed,
            "router should route to proxy_remote after successful refresh"
        );
        assert!(
            request_count.load(Ordering::SeqCst) >= 5,
            "should have retried 3 times on first fetch and succeeded on second cycle"
        );
    }
}

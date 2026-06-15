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
}

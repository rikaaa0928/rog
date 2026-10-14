use super::table::{IpDomainTable, MappingSnapshot, TableSnapshot};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
};
use std::{
    io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
struct ObserveState {
    table: Option<Arc<IpDomainTable>>,
    token: Option<String>,
}
#[derive(Default, serde::Deserialize)]
struct MappingQuery {
    router: Option<String>,
    client_ip: Option<IpAddr>,
    target_ip: Option<IpAddr>,
    domain: Option<String>,
    search: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}
#[derive(serde::Serialize)]
struct MappingResponse {
    #[serde(flatten)]
    snapshot: TableSnapshot,
    total: usize,
    offset: usize,
    limit: usize,
}
pub(crate) fn router(
    registry: proxy_observe::ObserveRegistry,
    table: Option<Arc<IpDomainTable>>,
    token: Option<String>,
) -> Router {
    let mapping = Router::new()
        .route("/api/v1/observe/ip-domains", get(mappings))
        .with_state(ObserveState {
            table,
            token: token.clone(),
        });
    proxy_observe::api::router_with_token(registry, token).merge(mapping)
}
pub(crate) async fn serve(
    router: Router,
    listen: SocketAddr,
    token: CancellationToken,
) -> io::Result<()> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(token.cancelled_owned())
        .await
}
async fn mappings(
    State(state): State<ObserveState>,
    headers: HeaderMap,
    Query(query): Query<MappingQuery>,
) -> Result<Json<MappingResponse>, StatusCode> {
    if !proxy_observe::api::is_authorized(&headers, state.token.as_deref()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let mut snapshot = match &state.table {
        Some(t) => t.snapshot(),
        None => TableSnapshot {
            enabled: false,
            key_count: 0,
            record_count: 0,
            capacity: 0,
            max_domains_per_ip: 0,
            max_ttl_secs: 0,
            entries: vec![],
        },
    };
    let domain = query.domain.as_deref().map(super::table::canonical);
    let search = query.search.as_deref().unwrap_or("").to_ascii_lowercase();
    snapshot
        .entries
        .retain(|r| matches_filter(r, &query, domain.as_deref(), &search));
    let total = snapshot.entries.len();
    let offset = query.offset.unwrap_or(0).min(total);
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    snapshot.entries = snapshot
        .entries
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect();
    Ok(Json(MappingResponse {
        snapshot,
        total,
        offset,
        limit,
    }))
}
fn matches_filter(
    row: &MappingSnapshot,
    q: &MappingQuery,
    domain: Option<&str>,
    search: &str,
) -> bool {
    q.router.as_ref().is_none_or(|r| r == &row.router)
        && q.client_ip
            .is_none_or(|ip| super::table::normalize(ip) == row.client_ip)
        && q.target_ip
            .is_none_or(|ip| super::table::normalize(ip) == row.target_ip)
        && domain.is_none_or(|d| row.domain.contains(d))
        && (search.is_empty()
            || row.router.to_ascii_lowercase().contains(search)
            || row.client_ip.to_string().contains(search)
            || row.target_ip.to_string().contains(search)
            || row.domain.contains(search))
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;
    #[tokio::test]
    async fn authorization_pagination_filters_and_legacy_endpoints() {
        let table = Arc::new(IpDomainTable::new(10, 10, 100));
        for name in ["a.example", "b.example"] {
            table.record(
                "main",
                "192.168.0.2".parse().unwrap(),
                "1.1.1.1".parse().unwrap(),
                name,
                60,
                std::time::Instant::now(),
            );
        }
        let app = router(
            proxy_observe::ObserveRegistry::new(),
            Some(table),
            Some("test-secret".into()),
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/observe/ip-domains")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/observe/ip-domains?limit=1&offset=1")
                    .header("authorization", "Bearer test-secret")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = serde_json::from_slice::<serde_json::Value>(
            &axum::body::to_bytes(response.into_body(), 10000)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["total"], 2);
        assert_eq!(body["entries"][0]["domain"], "b.example");
        assert_eq!(body["key_count"], 1);
        assert!(body["entries"][0]["ttl_remaining_ms"].as_u64().unwrap() > 0);
        for uri in [
            "/api/v1/observe/ip-domains?search=NO_MATCH",
            "/api/v1/observe/ip-domains?client_ip=192.168.0.3",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("x-proxy-observe-token", "test-secret")
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = serde_json::from_slice::<serde_json::Value>(
                &axum::body::to_bytes(response.into_body(), 10000)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(body["total"], 0);
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/observe/overview")
                    .header("authorization", "Bearer test-secret")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    #[tokio::test]
    async fn disabled_dns_is_explicit() {
        let app = router(proxy_observe::ObserveRegistry::new(), None, None);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/observe/ip-domains")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = serde_json::from_slice::<serde_json::Value>(
            &axum::body::to_bytes(response.into_body(), 10000)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["enabled"], false);
    }
}

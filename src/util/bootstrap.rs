//! Proxy-node DNS bootstrap uses explicit direct DNS, never the routed DNS service.
use hickory_resolver::{
    config::{NameServerConfig, ResolverConfig},
    name_server::TokioConnectionProvider,
    proto::xfer::Protocol,
};
use std::{
    collections::HashMap,
    io,
    net::{IpAddr, SocketAddr},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::net::TcpStream;
use tokio::time::timeout;

type BootstrapCacheKey = (String, Vec<SocketAddr>);
type BootstrapCacheMap = HashMap<BootstrapCacheKey, (Vec<IpAddr>, Instant)>;

static BOOTSTRAP_DNS_CACHE: OnceLock<Mutex<BootstrapCacheMap>> = OnceLock::new();
const BOOTSTRAP_CACHE_TTL: Duration = Duration::from_secs(60);

fn dns_cache() -> &'static Mutex<BootstrapCacheMap> {
    BOOTSTRAP_DNS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
pub(crate) fn servers(
    options: &Option<HashMap<String, toml::Value>>,
) -> io::Result<Option<Vec<SocketAddr>>> {
    let Some(value) = options.as_ref().and_then(|o| o.get("bootstrap_dns")) else {
        return Ok(None);
    };
    let values = value.as_array().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "bootstrap_dns must be a nonempty server array",
        )
    })?;
    let names = values
        .iter()
        .map(|v| {
            v.as_str().map(str::to_owned).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid bootstrap DNS server")
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    let parsed = crate::dns::config::servers(&names)?;
    if parsed.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "bootstrap_dns must not be empty",
        ));
    }
    Ok(Some(parsed))
}
pub(crate) async fn resolve(
    host: &str,
    port: u16,
    servers: &[SocketAddr],
) -> io::Result<Vec<SocketAddr>> {
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let cache_key = (host.to_ascii_lowercase(), servers.to_vec());
    let mut stale_ips: Option<Vec<IpAddr>> = None;
    if let Ok(guard) = dns_cache().lock()
        && let Some((ips, ts)) = guard.get(&cache_key)
    {
        if ts.elapsed() < BOOTSTRAP_CACHE_TTL {
            return Ok(ips.iter().map(|&ip| SocketAddr::new(ip, port)).collect());
        }
        stale_ips = Some(ips.clone());
    }

    for server in servers {
        let mut cfg = ResolverConfig::new();
        cfg.add_name_server(NameServerConfig {
            socket_addr: *server,
            protocol: Protocol::Udp,
            tls_dns_name: None,
            http_endpoint: None,
            trust_negative_responses: false,
            bind_addr: None,
        });
        let mut builder = hickory_resolver::Resolver::builder_with_config(
            cfg,
            TokioConnectionProvider::default(),
        );
        builder.options_mut().use_hosts_file = hickory_resolver::config::ResolveHosts::Never;
        builder.options_mut().timeout = Duration::from_secs(2);
        builder.options_mut().attempts = 1;
        let resolver = builder.build();
        if let Ok(lookup) = resolver.ipv4_lookup(host).await {
            let ips: Vec<IpAddr> = lookup.iter().map(|ip| IpAddr::V4(ip.0)).collect();
            if !ips.is_empty() {
                if let Ok(mut guard) = dns_cache().lock() {
                    guard.insert(cache_key, (ips.clone(), Instant::now()));
                }
                return Ok(ips
                    .into_iter()
                    .map(|ip| SocketAddr::new(ip, port))
                    .collect());
            }
        }
    }

    if let Some(ips) = stale_ips {
        return Ok(ips
            .into_iter()
            .map(|ip| SocketAddr::new(ip, port))
            .collect());
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "proxy endpoint bootstrap DNS failed",
    ))
}
pub(crate) async fn connect(
    host: &str,
    port: u16,
    servers: Option<&[SocketAddr]>,
) -> io::Result<TcpStream> {
    let connect_timeout = Duration::from_secs(5);
    let Some(servers) = servers else {
        return timeout(connect_timeout, TcpStream::connect((host, port)))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "proxy endpoint connect timeout")
            })?;
    };
    let mut error = io::Error::new(io::ErrorKind::NotFound, "no proxy endpoint address");
    for address in resolve(host, port, servers).await? {
        match timeout(connect_timeout, TcpStream::connect(address)).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => error = e,
            Err(_) => {
                error = io::Error::new(io::ErrorKind::TimedOut, "proxy endpoint connect timeout")
            }
        }
    }
    Err(error)
}
pub(crate) async fn connect_endpoint(
    endpoint: &str,
    options: &Option<HashMap<String, toml::Value>>,
) -> io::Result<TcpStream> {
    let connect_timeout = options
        .as_ref()
        .and_then(|o| o.get("connect_timeout_secs"))
        .and_then(|v| v.as_integer())
        .filter(|&v| v > 0)
        .map(|v| Duration::from_secs(v as u64))
        .unwrap_or(Duration::from_secs(5));
    let servers = servers(options)?;
    if let Ok(address) = endpoint.parse::<SocketAddr>() {
        return timeout(connect_timeout, TcpStream::connect(address))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "proxy endpoint connect timeout")
            })?;
    }
    let (host, port) = crate::connector::grpc::parse_address(endpoint)?;
    timeout(
        connect_timeout + Duration::from_secs(4),
        connect(&host, port, servers.as_deref()),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy endpoint connect timeout"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::{
        op::{Message, MessageType},
        rr::{RData, Record, rdata::A},
    };
    #[tokio::test]
    async fn explicit_direct_dns_resolves_and_connects_without_system_lookup() {
        let dns = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dns_addr = dns.local_addr().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut buffer = vec![0; 65535];
            loop {
                let (n, client) = dns.recv_from(&mut buffer).await.unwrap();
                let query = Message::from_vec(&buffer[..n]).unwrap();
                let mut response = Message::new();
                response
                    .set_id(query.id())
                    .set_message_type(MessageType::Response)
                    .set_recursion_available(true)
                    .set_recursion_desired(true)
                    .add_query(query.queries()[0].clone());
                response.add_answer(Record::from_rdata(
                    query.queries()[0].name().clone(),
                    60,
                    RData::A(A("127.0.0.1".parse().unwrap())),
                ));
                dns.send_to(&response.to_vec().unwrap(), client)
                    .await
                    .unwrap();
            }
        });
        let options = toml::from_str::<HashMap<String, toml::Value>>(&format!(
            "bootstrap_dns=['{dns_addr}']"
        ))
        .unwrap();
        let stream = connect_endpoint(
            &format!("node.bootstrap.example.com:{port}"),
            &Some(options),
        )
        .await
        .unwrap();
        assert_eq!(stream.peer_addr().unwrap(), listener.local_addr().unwrap());
        task.abort();
    }
    #[test]
    fn invalid_bootstrap_is_rejected() {
        let opts = toml::from_str::<HashMap<String, toml::Value>>("bootstrap_dns=[]").unwrap();
        assert!(servers(&Some(opts)).is_err());
    }
}

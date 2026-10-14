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
    time::Duration,
};
use tokio::net::TcpStream;
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
            let addresses: Vec<_> = lookup
                .iter()
                .map(|ip| SocketAddr::new(IpAddr::V4(ip.0), port))
                .collect();
            if !addresses.is_empty() {
                return Ok(addresses);
            }
        }
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
    let Some(servers) = servers else {
        return TcpStream::connect((host, port)).await;
    };
    let mut error = io::Error::new(io::ErrorKind::NotFound, "no proxy endpoint address");
    for address in resolve(host, port, servers).await? {
        match TcpStream::connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(e) => error = e,
        }
    }
    Err(error)
}
pub(crate) async fn connect_endpoint(
    endpoint: &str,
    options: &Option<HashMap<String, toml::Value>>,
) -> io::Result<TcpStream> {
    let servers = servers(options)?;
    if let Ok(address) = endpoint.parse::<SocketAddr>() {
        return TcpStream::connect(address).await;
    }
    let (host, port) = crate::connector::grpc::parse_address(endpoint)?;
    connect(&host, port, servers.as_deref()).await
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

use super::{
    config::{DnsConfig, servers},
    message,
    table::{IpDomainTable, canonical},
};
use crate::{
    connector,
    def::{RouterSet, RunConnector, UDPMeta, UDPPacket, config::Config},
};
use hickory_proto::op::{Message, MessageType, ResponseCode};
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::{
    net::UdpSocket,
    sync::{Mutex, Semaphore},
    task::JoinSet,
    time::{Duration, timeout_at},
};
use tokio_util::sync::CancellationToken;

type ConnectorCache = Mutex<HashMap<String, Arc<Box<dyn RunConnector>>>>;
#[derive(Default)]
struct Counters {
    received: AtomicU64,
    overloaded: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
}
pub(crate) struct DnsServer {
    socket: Arc<UdpSocket>,
    cfg: DnsConfig,
    config: Config,
    router: Arc<dyn RouterSet>,
    table: Arc<IpDomainTable>,
    cache: ConnectorCache,
    counters: Counters,
}
impl DnsServer {
    pub(crate) async fn bind(
        cfg: DnsConfig,
        config: Config,
        router: Arc<dyn RouterSet>,
        table: Arc<IpDomainTable>,
    ) -> io::Result<Arc<Self>> {
        let bind_addr: SocketAddr = cfg
            .endpoint
            .parse()
            .map_err(|_| super::config::invalid("invalid DNS endpoint"))?;
        let sock = socket2::Socket::new(
            socket2::Domain::for_address(bind_addr),
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        if bind_addr.ip().is_unspecified() {
            sock.set_reuse_address(true)?;
        }
        if bind_addr.is_ipv6() {
            let _ = sock.set_only_v6(false);
        }
        sock.set_nonblocking(true)?;
        sock.bind(&bind_addr.into())?;
        let socket = UdpSocket::from_std(sock.into())?;
        log::info!(
            "DNS listening on {} router {}",
            socket.local_addr()?,
            cfg.router
        );
        Ok(Arc::new(Self {
            socket: Arc::new(socket),
            cfg,
            config,
            router,
            table,
            cache: Mutex::new(HashMap::new()),
            counters: Counters::default(),
        }))
    }
    pub(crate) async fn run(self: Arc<Self>, shutdown: CancellationToken) -> io::Result<()> {
        let permits = Arc::new(Semaphore::new(self.cfg.max_concurrent_requests));
        let mut tasks = JoinSet::new();
        let mut buffer = vec![0; 65535];
        let mut cleanup =
            tokio::time::interval(Duration::from_secs(self.cfg.mapping_cleanup_interval_secs));
        let result = loop {
            tokio::select! {
                biased;
                _=shutdown.cancelled()=>break Ok(()),
                _=cleanup.tick()=>self.table.prune_expired(),
                Some(result)=tasks.join_next(), if !tasks.is_empty()=>{ if let Err(error)=result { log::warn!("DNS request task failed: {error}"); } },
                received=self.socket.recv_from(&mut buffer)=>{
                    let (len,client)=match received { Ok(v)=>v,Err(e)=>break Err(e) };
                    self.counters.received.fetch_add(1,Ordering::Relaxed);
                    let permit=match permits.clone().try_acquire_owned(){Ok(p)=>p,Err(_)=>{self.counters.overloaded.fetch_add(1,Ordering::Relaxed);continue}};
                    let data=buffer[..len].to_vec(); let server=self.clone();
                    tasks.spawn(async move { let _permit=permit; server.handle(client,data).await; });
                }
            }
        };
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        log::info!(
            "DNS stopped received={} completed={} failed={} overloaded={}",
            self.counters.received.load(Ordering::Relaxed),
            self.counters.completed.load(Ordering::Relaxed),
            self.counters.failed.load(Ordering::Relaxed),
            self.counters.overloaded.load(Ordering::Relaxed)
        );
        result
    }
    async fn handle(&self, client: SocketAddr, data: Vec<u8>) {
        let query = match Message::from_vec(&data) {
            Ok(q) => q,
            Err(_) => {
                if data.len() >= 12 && data[2] & 0x80 == 0 {
                    let mut q = Message::new();
                    q.set_id(u16::from_be_bytes([data[0], data[1]]))
                        .set_recursion_desired(data[2] & 1 != 0);
                    let _ = self
                        .socket
                        .send_to(&message::error_reply(&q, ResponseCode::FormErr), client)
                        .await;
                }
                return;
            }
        };
        if query.message_type() != MessageType::Query {
            return;
        }
        if let Some(code) = message::query_error(&query) {
            let _ = self
                .socket
                .send_to(&message::error_reply(&query, code), client)
                .await;
            return;
        }
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.cfg.request_timeout_ms);
        let result = timeout_at(deadline, self.forward(client, &data, &query, deadline)).await;
        let reply = match result {
            Ok(Ok(data)) => {
                self.counters.completed.fetch_add(1, Ordering::Relaxed);
                data
            }
            error => {
                self.counters.failed.fetch_add(1, Ordering::Relaxed);
                log::debug!("DNS failed client={client} error={error:?}");
                message::error_reply(&query, ResponseCode::ServFail)
            }
        };
        if let Err(e) = self.socket.send_to(&reply, client).await {
            log::debug!("DNS client reply failed: {e}");
        }
    }
    async fn forward(
        &self,
        client: SocketAddr,
        bytes: &[u8],
        query: &Message,
        deadline: tokio::time::Instant,
    ) -> io::Result<Vec<u8>> {
        let domain = canonical(&query.query().unwrap().name().to_ascii());
        let decision = self.router.route_dns(&self.cfg.router, &domain).await?;
        let cfg = self
            .config
            .connector
            .iter()
            .find(|c| c.name == decision.outbound)
            .ok_or_else(|| io::Error::other("DNS connector not found"))?;
        if cfg.proto == "block" {
            return Ok(message::error_reply(query, ResponseCode::Refused));
        }
        if self.cfg.disable_aaaa
            && query
                .query()
                .is_some_and(|q| q.query_type() == hickory_proto::rr::RecordType::AAAA)
        {
            return Ok(message::error_reply(query, ResponseCode::NoError));
        }
        let chosen = if decision.matched_rule.is_some() {
            cfg.dns
                .as_ref()
                .filter(|v| !v.is_empty())
                .unwrap_or(&self.cfg.default_servers)
        } else {
            &self.cfg.default_servers
        };
        log::debug!(
            "DNS client={client} domain={domain} rule={:?} outbound={}",
            decision.matched_rule,
            decision.outbound
        );
        for upstream in servers(chosen)? {
            let attempt_deadline = (tokio::time::Instant::now()
                + Duration::from_millis(self.cfg.upstream_timeout_ms))
            .min(deadline);
            let attempt = async {
                let cached = { self.cache.lock().await.get(&decision.outbound).cloned() };
                let connector = match cached {
                    Some(c) => c,
                    None => {
                        let fresh = Arc::new(connector::create(cfg).await?);
                        let mut cache = self.cache.lock().await;
                        cache
                            .entry(decision.outbound.clone())
                            .or_insert(fresh)
                            .clone()
                    }
                };
                let norm_client = SocketAddr::new(super::table::normalize(client.ip()), client.port());
                let (mut reader, writer) = connector
                    .udp_tunnel(norm_client.to_string())
                    .await?
                    .ok_or_else(|| io::Error::other("DNS connector has no UDP tunnel"))?;
                writer
                    .write(UDPPacket {
                        meta: UDPMeta {
                            src_addr: norm_client.ip().to_string(),
                            src_port: norm_client.port(),
                            dst_addr: upstream.ip().to_string(),
                            dst_port: upstream.port(),
                        },
                        data: bytes.to_vec(),
                    })
                    .await?;
                loop {
                    let packet = reader.read().await?;
                    if packet.meta.dst_addr.parse::<std::net::IpAddr>().ok() != Some(upstream.ip())
                        || packet.meta.dst_port != upstream.port()
                    {
                        continue;
                    }
                    if packet.data.len() > 65535 {
                        continue;
                    }
                    let response = match Message::from_vec(&packet.data) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    if !message::valid_response(query, &response) {
                        continue;
                    }
                    if !response.truncated()
                        && matches!(
                            response.response_code(),
                            ResponseCode::ServFail | ResponseCode::Refused
                        )
                    {
                        return Err(io::Error::other("DNS upstream failed"));
                    }
                    let received = Instant::now();
                    for (ip, ttl) in message::addresses(query, &response) {
                        self.table.record(
                            &self.cfg.router,
                            client.ip(),
                            ip,
                            &domain,
                            ttl,
                            received,
                        );
                    }
                    return Ok::<_, io::Error>(packet.data);
                }
            };
            match timeout_at(attempt_deadline, attempt).await {
                Ok(Ok(data)) => return Ok(data),
                result => log::debug!(
                    "DNS upstream={upstream} outbound={} failed={result:?}",
                    decision.outbound
                ),
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "all DNS upstreams failed",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{DefaultRouter, resolver::Resolver};
    use hickory_proto::{
        op::Query,
        rr::{Name, RData, Record, RecordType, rdata::A},
    };
    async fn mock(ip: &str, code: ResponseCode) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let ip = ip.parse().unwrap();
        let task = tokio::spawn(async move {
            let mut buf = vec![0; 65535];
            loop {
                let (n, c) = socket.recv_from(&mut buf).await.unwrap();
                let q = Message::from_vec(&buf[..n]).unwrap();
                let mut r = Message::new();
                r.set_id(q.id())
                    .set_message_type(MessageType::Response)
                    .set_response_code(code)
                    .add_query(q.queries()[0].clone());
                if code == ResponseCode::NoError {
                    r.add_answer(Record::from_rdata(
                        q.queries()[0].name().clone(),
                        60,
                        RData::A(A(ip)),
                    ));
                }
                socket.send_to(&r.to_vec().unwrap(), c).await.unwrap();
            }
        });
        (addr, task)
    }
    async fn send(socket: &UdpSocket, dns: SocketAddr, name: &str, id: u16) -> Message {
        let mut q = Message::new();
        q.set_id(id)
            .add_query(Query::query(Name::from_ascii(name).unwrap(), RecordType::A));
        socket.send_to(&q.to_vec().unwrap(), dns).await.unwrap();
        let mut buf = vec![0; 65535];
        let (n, _) = tokio::time::timeout(Duration::from_secs(3), socket.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        Message::from_vec(&buf[..n]).unwrap()
    }
    fn configuration(first: SocketAddr, second: SocketAddr) -> Config {
        toml::from_str(&format!(
            r#"
listener=[]
[dns]
endpoint="127.0.0.1:5353"
router="main"
default_servers=["{first}","{second}"]
upstream_timeout_ms=40
request_timeout_ms=400
[[connector]]
name="default"
proto="tcp"
dns=["{second}"]
[[connector]]
name="other"
proto="tcp"
dns=["{second}"]
[[connector]]
name="deny"
proto="block"
[[data]]
name="names"
format="domain-suffix"
data="other.example"
[[data]]
name="blocked"
format="domain-suffix"
data="blocked.example"
[[router]]
name="main"
default="default"
[[router.route_rules]]
name="names"
select="other"
exclude=[]
[[router.route_rules]]
name="blocked"
select="deny"
exclude=[]
"#
        ))
        .unwrap()
    }
    #[tokio::test]
    async fn per_request_routing_learning_and_shutdown() {
        let (first, a) = mock("1.1.1.1", ResponseCode::NoError).await;
        let (second, b) = mock("8.8.8.8", ResponseCode::NoError).await;
        let cfg = configuration(first, second);
        cfg.validate().unwrap();
        let table = Arc::new(IpDomainTable::new(100, 10, 100));
        let mut router =
            DefaultRouter::new(&cfg.router, cfg.data.as_ref().unwrap(), Resolver::new()).await;
        router.dns_table = Some(table.clone());
        let mut settings = cfg.dns.clone().unwrap();
        settings.endpoint = "127.0.0.1:0".into();
        let router = Arc::new(router);
        let server = DnsServer::bind(settings, cfg, router.clone(), table.clone())
            .await
            .unwrap();
        let addr = server.socket.local_addr().unwrap();
        let token = CancellationToken::new();
        let task = tokio::spawn(server.run(token.clone()));
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let one = send(&client, addr, "default.example", 7).await;
        let two = send(&client, addr, "other.example", 7).await;
        assert_eq!(
            one.answers()[0].data(),
            &RData::A(A("1.1.1.1".parse().unwrap()))
        );
        assert_eq!(
            two.answers()[0].data(),
            &RData::A(A("8.8.8.8".parse().unwrap()))
        );
        assert_eq!(
            table.get_domains(
                "main",
                client.local_addr().unwrap().ip(),
                "8.8.8.8".parse().unwrap()
            ),
            vec!["other.example"]
        );
        assert_eq!(
            send(&client, addr, "blocked.example", 8)
                .await
                .response_code(),
            ResponseCode::Refused
        );
        let mut aaaa = Message::new();
        aaaa.set_id(9).add_query(Query::query(
            Name::from_ascii("default.example").unwrap(),
            RecordType::AAAA,
        ));
        let mut cfg_no_aaaa = configuration(first, second);
        cfg_no_aaaa.dns.as_mut().unwrap().disable_aaaa = true;
        cfg_no_aaaa.dns.as_mut().unwrap().endpoint = "127.0.0.1:0".into();
        let srv_no_aaaa = DnsServer::bind(
            cfg_no_aaaa.dns.clone().unwrap(),
            cfg_no_aaaa,
            router.clone(),
            table.clone(),
        )
        .await
        .unwrap();
        let addr_no_aaaa = srv_no_aaaa.socket.local_addr().unwrap();
        let tok_no_aaaa = CancellationToken::new();
        let task_no_aaaa = tokio::spawn(srv_no_aaaa.run(tok_no_aaaa.clone()));
        client
            .send_to(&aaaa.to_vec().unwrap(), addr_no_aaaa)
            .await
            .unwrap();
        let mut buf = vec![0; 512];
        let (n, _) = client.recv_from(&mut buf).await.unwrap();
        let resp_aaaa = Message::from_vec(&buf[..n]).unwrap();
        assert_eq!(resp_aaaa.response_code(), ResponseCode::NoError);
        assert!(resp_aaaa.answers().is_empty());
        tok_no_aaaa.cancel();
        let _ = task_no_aaaa.await;
        let route = router
            .route_with_context(
                "tproxy",
                "main",
                &crate::util::RunAddr {
                    addr: "8.8.8.8".into(),
                    port: 443,
                    udp: false,
                },
                &crate::def::RouteContext {
                    client_ip: Some(client.local_addr().unwrap().ip()),
                    use_dns_mapping: true,
                },
            )
            .await;
        assert_eq!(route, "other");
        let client2 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (left, right) = tokio::join!(
            send(&client, addr, "other.example", 10),
            send(&client2, addr, "default.example", 10)
        );
        assert_eq!(
            left.answers()[0].data(),
            &RData::A(A("8.8.8.8".parse().unwrap()))
        );
        assert_eq!(
            right.answers()[0].data(),
            &RData::A(A("1.1.1.1".parse().unwrap()))
        );
        token.cancel();
        task.await.unwrap().unwrap();
        assert!(UdpSocket::bind(addr).await.is_ok());
        a.abort();
        b.abort();
    }
    #[tokio::test]
    async fn failover_nxdomain_and_servfail() {
        let (first, a) = mock("1.1.1.1", ResponseCode::ServFail).await;
        let (second, b) = mock("8.8.8.8", ResponseCode::NXDomain).await;
        let cfg = configuration(first, second);
        let router = Arc::new(
            DefaultRouter::new(&cfg.router, cfg.data.as_ref().unwrap(), Resolver::new()).await,
        );
        let table = Arc::new(IpDomainTable::new(10, 10, 100));
        let mut settings = cfg.dns.clone().unwrap();
        settings.endpoint = "127.0.0.1:0".into();
        let server = DnsServer::bind(settings, cfg, router, table).await.unwrap();
        let addr = server.socket.local_addr().unwrap();
        let token = CancellationToken::new();
        let task = tokio::spawn(server.run(token.clone()));
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        assert_eq!(
            send(&client, addr, "default.example", 1)
                .await
                .response_code(),
            ResponseCode::NXDomain
        );
        a.abort();
        b.abort();
        assert_eq!(
            send(&client, addr, "default.example", 2)
                .await
                .response_code(),
            ResponseCode::ServFail
        );
        token.cancel();
        task.await.unwrap().unwrap();
    }
    #[tokio::test]
    async fn runtime_bind_failure_and_cancel() {
        let occupied = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = configuration("1.1.1.1:53".parse().unwrap(), "8.8.8.8:53".parse().unwrap());
        cfg.dns.as_mut().unwrap().endpoint = occupied.local_addr().unwrap().to_string();
        let token = CancellationToken::new();
        let error = crate::run(crate::RunOptions {
            config: cfg.clone(),
            observe_registry: proxy_observe::ObserveRegistry::new(),
            shutdown: token.clone(),
        })
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        drop(occupied);
        token.cancel();
        tokio::time::timeout(
            Duration::from_secs(2),
            crate::run(crate::RunOptions {
                config: cfg,
                observe_registry: proxy_observe::ObserveRegistry::new(),
                shutdown: token,
            }),
        )
        .await
        .unwrap()
        .unwrap();
    }
    #[tokio::test]
    async fn overload_is_bounded_and_cancel_releases_socket() {
        let upstream = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let cfg = configuration(
            upstream.local_addr().unwrap(),
            upstream.local_addr().unwrap(),
        );
        let router = Arc::new(
            DefaultRouter::new(&cfg.router, cfg.data.as_ref().unwrap(), Resolver::new()).await,
        );
        let mut settings = cfg.dns.clone().unwrap();
        settings.endpoint = "127.0.0.1:0".into();
        settings.max_concurrent_requests = 1;
        settings.upstream_timeout_ms = 5000;
        let server = DnsServer::bind(
            settings,
            cfg,
            router,
            Arc::new(IpDomainTable::new(10, 10, 60)),
        )
        .await
        .unwrap();
        let addr = server.socket.local_addr().unwrap();
        let token = CancellationToken::new();
        let task = tokio::spawn(server.clone().run(token.clone()));
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut query = Message::new();
        query.add_query(Query::query(
            Name::from_ascii("default.example").unwrap(),
            RecordType::A,
        ));
        let bytes = query.to_vec().unwrap();
        client.send_to(&bytes, addr).await.unwrap();
        let mut buf = [0; 1024];
        tokio::time::timeout(Duration::from_secs(1), upstream.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        client.send_to(&bytes, addr).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while server.counters.overloaded.load(Ordering::Relaxed) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        token.cancel();
        task.await.unwrap().unwrap();
        drop(server);
        assert!(UdpSocket::bind(addr).await.is_ok());
    }
}

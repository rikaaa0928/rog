use super::{
    config::{DnsConfig, servers},
    message,
    table::{IpDomainTable, canonical},
};
use crate::{
    connector,
    def::{RouterSet, RunConnector, UDPMeta, UDPPacket, config::Config},
};
use hickory_proto::{
    op::{Message, MessageType, ResponseCode},
    rr::{DNSClass, RData, Record, RecordType},
};
use lru::LruCache;
use std::{
    collections::HashMap,
    io,
    net::{IpAddr, SocketAddr},
    num::NonZeroUsize,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::{
    net::UdpSocket,
    sync::{Mutex, Semaphore, watch},
    task::{JoinHandle, JoinSet},
    time::{Duration, timeout_at},
};
use tokio_util::sync::CancellationToken;

const MAX_NODATA_CACHE_SECS: u64 = 300;
const STALE_GRACE_SECS: u64 = 1800;
const STALE_REFRESH_GRACE_MS: u64 = 250;
const STALE_REPLY_TTL_SECS: u32 = 30;
const HEDGE_MAX_DELAY_MS: u64 = 250;
const MAX_BG_PREFETCH_TASKS: usize = 32;

type ConnectorCache = Mutex<HashMap<String, Arc<Box<dyn RunConnector>>>>;

#[derive(Clone, Hash, PartialEq, Eq)]
struct ResponseCacheKey {
    domain: String,
    qtype: RecordType,
    qclass: DNSClass,
    outbound: String,
    recursion_desired: bool,
    has_edns: bool,
    checking_disabled: bool,
    dnssec_ok: bool,
}

#[derive(Clone)]
struct CachedResponse {
    response: Message,
    cnames: Vec<(String, u32)>,
    addresses: Vec<(IpAddr, u32)>,
    inserted_at: Instant,
    fresh_ttl: Duration,
    stale_until: Instant,
}

#[derive(Clone)]
enum InflightState {
    Pending,
    Cached(CachedResponse),
    Raw(Vec<u8>),
    Failed,
}

struct InflightGuard {
    inflight: Arc<StdMutex<HashMap<ResponseCacheKey, watch::Sender<InflightState>>>>,
    key: ResponseCacheKey,
    tx: watch::Sender<InflightState>,
    completed: bool,
}

impl InflightGuard {
    fn complete(&mut self, state: InflightState) {
        self.completed = true;
        let _ = self.tx.send(state);
        let mut map = self.inflight.lock().unwrap();
        if map
            .get(&self.key)
            .is_some_and(|cur| cur.same_channel(&self.tx))
        {
            map.remove(&self.key);
        }
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.tx.send(InflightState::Failed);
            let mut map = self.inflight.lock().unwrap();
            if map
                .get(&self.key)
                .is_some_and(|cur| cur.same_channel(&self.tx))
            {
                map.remove(&self.key);
            }
        }
    }
}

#[derive(Default)]
struct Counters {
    received: AtomicU64,
    overloaded: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
    cache_hits: AtomicU64,
    stale_hits: AtomicU64,
}
pub(crate) struct DnsServer {
    socket: Arc<UdpSocket>,
    cfg: DnsConfig,
    config: Config,
    ebpf_direct_connectors: std::collections::HashSet<String>,
    router: Arc<dyn RouterSet>,
    table: Arc<IpDomainTable>,
    cache: ConnectorCache,
    response_cache: StdMutex<LruCache<ResponseCacheKey, CachedResponse>>,
    inflight: Arc<StdMutex<HashMap<ResponseCacheKey, watch::Sender<InflightState>>>>,
    prefetch_permits: Arc<Semaphore>,
    bg_shutdown: CancellationToken,
    bg_handles: StdMutex<Vec<JoinHandle<()>>>,
    counters: Counters,
}
impl DnsServer {
    pub(crate) async fn bind(
        cfg: DnsConfig,
        mut config: Config,
        router: Arc<dyn RouterSet>,
        table: Arc<IpDomainTable>,
    ) -> io::Result<Arc<Self>> {
        config.apply_defaults();
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
        let ebpf_direct_connectors = config.resolve_ebpf_direct_connectors();
        let cache_cap = NonZeroUsize::new(cfg.mapping_capacity.clamp(256, 16384)).unwrap();
        Ok(Arc::new(Self {
            socket: Arc::new(socket),
            cfg,
            config,
            ebpf_direct_connectors,
            router,
            table,
            cache: Mutex::new(HashMap::new()),
            response_cache: StdMutex::new(LruCache::new(cache_cap)),
            inflight: Arc::new(StdMutex::new(HashMap::new())),
            prefetch_permits: Arc::new(Semaphore::new(MAX_BG_PREFETCH_TASKS)),
            bg_shutdown: CancellationToken::new(),
            bg_handles: StdMutex::new(Vec::new()),
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
                _=cleanup.tick()=>{
                    self.table.prune_expired();
                    self.prune_response_cache();
                },
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
        self.bg_shutdown.cancel();
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        let handles = std::mem::take(&mut *self.bg_handles.lock().unwrap());
        for handle in handles {
            handle.abort();
            let _ = handle.await;
        }
        log::info!(
            "DNS stopped received={} completed={} failed={} overloaded={} cache_hits={} stale_hits={}",
            self.counters.received.load(Ordering::Relaxed),
            self.counters.completed.load(Ordering::Relaxed),
            self.counters.failed.load(Ordering::Relaxed),
            self.counters.overloaded.load(Ordering::Relaxed),
            self.counters.cache_hits.load(Ordering::Relaxed),
            self.counters.stale_hits.load(Ordering::Relaxed)
        );
        result
    }
    async fn handle(self: Arc<Self>, client: SocketAddr, data: Vec<u8>) {
        crate::util::ra_suppress::note_client_ip(client.ip());
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
        let result = timeout_at(
            deadline,
            self.clone().forward(client, &data, &query, deadline),
        )
        .await;
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
    fn cache_key(
        query: &Message,
        domain: &str,
        outbound_name: &str,
    ) -> Option<ResponseCacheKey> {
        let q = query.query()?;
        let edns = query.extensions().as_ref();
        if edns.is_some_and(|e| !e.options().as_ref().is_empty()) {
            return None;
        }
        Some(ResponseCacheKey {
            domain: domain.to_string(),
            qtype: q.query_type(),
            qclass: q.query_class(),
            outbound: outbound_name.to_string(),
            recursion_desired: query.recursion_desired(),
            has_edns: edns.is_some(),
            checking_disabled: query.checking_disabled(),
            dnssec_ok: edns.is_some_and(|e| e.flags().dnssec_ok),
        })
    }
    fn cache_fresh_ttl(&self, response: &Message) -> Option<Duration> {
        if response.truncated() || response.response_code() != ResponseCode::NoError {
            return None;
        }
        let max_ttl = self.cfg.mapping_max_ttl_secs;
        if !response.answers().is_empty() {
            let min_ttl = response.answers().iter().map(|r| r.ttl()).min()?;
            if min_ttl == 0 {
                return None;
            }
            return Some(Duration::from_secs((min_ttl as u64).clamp(1, max_ttl)));
        }
        let soa_ttl = response
            .name_servers()
            .iter()
            .filter_map(|r| match r.data() {
                RData::SOA(soa) => Some(r.ttl().min(soa.minimum())),
                _ => None,
            })
            .min()?;
        if soa_ttl == 0 {
            return None;
        }
        Some(Duration::from_secs(
            (soa_ttl as u64).clamp(5, MAX_NODATA_CACHE_SECS.min(max_ttl)),
        ))
    }
    fn build_cached_reply(
        cached: &CachedResponse,
        query: &Message,
        now: Instant,
        is_stale: bool,
    ) -> Option<Vec<u8>> {
        let mut msg = cached.response.clone();
        msg.set_id(query.id());
        msg.set_recursion_desired(query.recursion_desired());
        if let Some(q) = query.query() {
            msg.take_queries();
            msg.add_query(q.clone());
        }
        let elapsed = now.saturating_duration_since(cached.inserted_at).as_secs() as u32;
        let adjust = |r: &mut Record| {
            if r.record_type() != RecordType::OPT {
                let ttl = if is_stale {
                    r.ttl().min(STALE_REPLY_TTL_SECS).max(1)
                } else {
                    r.ttl().saturating_sub(elapsed).max(1)
                };
                r.set_ttl(ttl);
            }
        };
        for r in msg.answers_mut() {
            adjust(r);
        }
        for r in msg.name_servers_mut() {
            adjust(r);
        }
        for r in msg.additionals_mut() {
            adjust(r);
        }
        if query.extensions().is_none() {
            *msg.extensions_mut() = None;
        }
        let bytes = msg.to_vec().ok()?;
        let max_payload = query
            .extensions()
            .as_ref()
            .map(|e| usize::from(e.max_payload().max(512)))
            .unwrap_or(512);
        if bytes.len() > max_payload {
            return None;
        }
        Some(bytes)
    }
    fn patch_raw_reply(mut raw: Vec<u8>, query: &Message) -> Vec<u8> {
        if raw.len() >= 12 {
            raw[0..2].copy_from_slice(&query.id().to_be_bytes());
            if query.recursion_desired() {
                raw[2] |= 1;
            } else {
                raw[2] &= !1;
            }
        }
        raw
    }
    fn prune_response_cache(&self) {
        let now = Instant::now();
        let mut cache = self.response_cache.lock().unwrap();
        let expired: Vec<_> = cache
            .iter()
            .filter_map(|(k, v)| (now > v.stale_until).then(|| k.clone()))
            .collect();
        for k in expired {
            cache.pop(&k);
        }
    }
    fn track_bg_handle(&self, handle: JoinHandle<()>) {
        let mut handles = self.bg_handles.lock().unwrap();
        handles.retain(|h| !h.is_finished());
        handles.push(handle);
    }
    async fn learn_records(
        &self,
        client: SocketAddr,
        domain: &str,
        cnames: &[(String, u32)],
        addresses: &[(IpAddr, u32)],
        received: Instant,
    ) {
        for (cname_target, cname_ttl) in cnames {
            let table_ttl = if *cname_ttl > 0 {
                (*cname_ttl).max(3600)
            } else {
                0
            };
            self.table.record_cname(
                &self.cfg.router,
                client.ip(),
                cname_target,
                domain,
                table_ttl,
                received,
            );
        }
        for &(ip, ttl) in addresses {
            let table_ttl = if ttl > 0 { ttl.max(3600) } else { 0 };
            self.table
                .record(&self.cfg.router, client.ip(), ip, domain, table_ttl, received);
            if table_ttl > 0 {
                let (effective_outbound, _) = self
                    .router
                    .route_with_domain(
                        "",
                        &self.cfg.router,
                        &crate::util::RunAddr {
                            addr: ip.to_string(),
                            port: 0,
                            udp: false,
                        },
                        &crate::def::RouteContext {
                            client_ip: Some(client.ip()),
                            client_port: Some(client.port()),
                            use_dns_mapping: true,
                        },
                    )
                    .await;
                let is_direct = self
                    .ebpf_direct_connectors
                    .contains(effective_outbound.as_str());
                if is_direct {
                    let ebpf_ttl = (table_ttl / 2).max(60);
                    crate::util::ebpf::offload_direct_ip(
                        ip,
                        Duration::from_secs(u64::from(ebpf_ttl)),
                    );
                } else {
                    crate::util::ebpf::remove_direct_ip(ip);
                }
            }
        }
    }
    async fn get_connector(
        &self,
        outbound_name: &str,
        cfg: &crate::def::config::Connector,
    ) -> io::Result<Arc<Box<dyn RunConnector>>> {
        let cached = { self.cache.lock().await.get(outbound_name).cloned() };
        match cached {
            Some(c) => Ok(c),
            None => {
                let fresh = Arc::new(connector::create(cfg).await?);
                let mut cache = self.cache.lock().await;
                Ok(cache
                    .entry(outbound_name.to_string())
                    .or_insert(fresh)
                    .clone())
            }
        }
    }
    async fn query_upstreams_hedged(
        &self,
        outbound_name: &str,
        cfg: &crate::def::config::Connector,
        chosen: &[String],
        client: SocketAddr,
        bytes: &[u8],
        query: &Message,
        deadline: tokio::time::Instant,
    ) -> io::Result<(Vec<u8>, Message)> {
        let upstreams = servers(chosen)?;
        if upstreams.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no DNS upstream configured",
            ));
        }
        let connector = self.get_connector(outbound_name, cfg).await?;
        let norm_client = SocketAddr::new(super::table::normalize(client.ip()), client.port());
        let hedge_delay =
            Duration::from_millis((self.cfg.upstream_timeout_ms / 4).clamp(1, HEDGE_MAX_DELAY_MS));
        let upstream_timeout = Duration::from_millis(self.cfg.upstream_timeout_ms);

        let mut tasks = JoinSet::new();
        let mut next_idx = 0usize;
        let spawn_attempt = |tasks: &mut JoinSet<io::Result<(Vec<u8>, Message)>>,
                             upstream: SocketAddr| {
            let connector = connector.clone();
            let bytes = bytes.to_vec();
            let query = query.clone();
            let attempt_deadline = (tokio::time::Instant::now() + upstream_timeout).min(deadline);
            tasks.spawn(async move {
                let fut = async {
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
                            data: bytes,
                        })
                        .await?;
                    loop {
                        let packet = reader.read().await?;
                        if packet.meta.dst_addr.parse::<IpAddr>().ok() != Some(upstream.ip())
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
                        if !message::valid_response(&query, &response) {
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
                        return Ok((packet.data, response));
                    }
                };
                match timeout_at(attempt_deadline, fut).await {
                    Ok(res) => res,
                    Err(_) => Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("DNS upstream {upstream} timed out"),
                    )),
                }
            });
        };

        spawn_attempt(&mut tasks, upstreams[next_idx]);
        next_idx += 1;
        let mut next_hedge_at = tokio::time::Instant::now() + hedge_delay;

        loop {
            if next_idx < upstreams.len() {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    // Deadline reached; wait on already-spawned tasks or abort below
                    while let Some(res) = tasks.join_next().await {
                        if let Ok(Ok(ok)) = res {
                            return Ok(ok);
                        }
                    }
                    break;
                }
                let sleep_until = next_hedge_at.min(deadline);
                tokio::select! {
                    biased;
                    joined = tasks.join_next(), if !tasks.is_empty() => {
                        match joined {
                            Some(Ok(Ok(ok))) => return Ok(ok),
                            Some(Ok(Err(e))) => {
                                log::debug!("DNS upstream attempt failed early outbound={outbound_name}: {e}");
                                spawn_attempt(&mut tasks, upstreams[next_idx]);
                                next_idx += 1;
                                next_hedge_at = tokio::time::Instant::now() + hedge_delay;
                            }
                            Some(Err(e)) => {
                                log::debug!("DNS upstream task join error: {e}");
                                spawn_attempt(&mut tasks, upstreams[next_idx]);
                                next_idx += 1;
                                next_hedge_at = tokio::time::Instant::now() + hedge_delay;
                            }
                            None => {}
                        }
                    }
                    _ = tokio::time::sleep_until(sleep_until) => {
                        if tokio::time::Instant::now() >= deadline {
                            break;
                        }
                        spawn_attempt(&mut tasks, upstreams[next_idx]);
                        next_idx += 1;
                        next_hedge_at = tokio::time::Instant::now() + hedge_delay;
                    }
                }
            } else {
                match tasks.join_next().await {
                    Some(Ok(Ok(ok))) => return Ok(ok),
                    Some(Ok(Err(e))) => {
                        log::debug!("DNS upstream attempt failed outbound={outbound_name}: {e}");
                    }
                    Some(Err(e)) => {
                        log::debug!("DNS upstream task join error: {e}");
                    }
                    None => break,
                }
            }
        }

        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "all DNS upstreams failed",
        ))
    }
    #[allow(clippy::too_many_arguments)]
    fn spawn_singleflight_leader(
        self: &Arc<Self>,
        key: ResponseCacheKey,
        outbound_name: String,
        outbound_cfg: crate::def::config::Connector,
        chosen: Vec<String>,
        client: SocketAddr,
        bytes: Vec<u8>,
        query: Message,
    ) -> watch::Receiver<InflightState> {
        let (tx, rx) = watch::channel(InflightState::Pending);
        {
            let mut map = self.inflight.lock().unwrap();
            if let Some(existing) = map.get(&key) {
                return existing.subscribe();
            }
            map.insert(key.clone(), tx.clone());
        }
        let server = self.clone();
        let inflight = self.inflight.clone();
        let shutdown = self.bg_shutdown.clone();
        let handle = tokio::spawn(async move {
            let mut guard = InflightGuard {
                inflight,
                key: key.clone(),
                tx,
                completed: false,
            };
            let leader_deadline =
                tokio::time::Instant::now() + Duration::from_millis(server.cfg.request_timeout_ms);
            let res = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                res = server.query_upstreams_hedged(
                    &outbound_name,
                    &outbound_cfg,
                    &chosen,
                    client,
                    &bytes,
                    &query,
                    leader_deadline,
                ) => res,
            };
            match res {
                Ok((raw_data, response)) => {
                    let cnames = message::cname_chain(&query, &response);
                    let addresses = message::addresses(&query, &response);
                    if let Some(fresh_ttl) = server.cache_fresh_ttl(&response) {
                        let now = Instant::now();
                        let stale_until = now
                            + fresh_ttl
                            + Duration::from_secs(STALE_GRACE_SECS);
                        let cached = CachedResponse {
                            response,
                            cnames,
                            addresses,
                            inserted_at: now,
                            fresh_ttl,
                            stale_until,
                        };
                        server.response_cache.lock().unwrap().put(key, cached.clone());
                        guard.complete(InflightState::Cached(cached));
                    } else {
                        guard.complete(InflightState::Raw(raw_data));
                    }
                }
                Err(_) => {
                    guard.complete(InflightState::Failed);
                }
            }
        });
        self.track_bg_handle(handle);
        rx
    }
    #[allow(clippy::too_many_arguments)]
    fn maybe_spawn_prefetch(
        self: &Arc<Self>,
        key: ResponseCacheKey,
        outbound_name: String,
        outbound_cfg: crate::def::config::Connector,
        chosen: Vec<String>,
        client: SocketAddr,
        bytes: Vec<u8>,
        query: Message,
    ) {
        if self.inflight.lock().unwrap().contains_key(&key) {
            return;
        }
        let Ok(permit) = self.prefetch_permits.clone().try_acquire_owned() else {
            return;
        };
        let mut rx = self.spawn_singleflight_leader(
            key,
            outbound_name,
            outbound_cfg,
            chosen,
            client,
            bytes,
            query,
        );
        let shutdown = self.bg_shutdown.clone();
        let handle = tokio::spawn(async move {
            let _permit = permit;
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {},
                _ = rx.wait_for(|v| !matches!(v, InflightState::Pending)) => {},
            }
        });
        self.track_bg_handle(handle);
    }
    async fn wait_singleflight(
        &self,
        mut rx: watch::Receiver<InflightState>,
        wait_deadline: tokio::time::Instant,
    ) -> io::Result<InflightState> {
        match timeout_at(
            wait_deadline,
            rx.wait_for(|v| !matches!(v, InflightState::Pending)),
        )
        .await
        {
            Ok(Ok(state)) => {
                let s = state.clone();
                if matches!(s, InflightState::Failed | InflightState::Pending) {
                    Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "all DNS upstreams failed",
                    ))
                } else {
                    Ok(s)
                }
            }
            _ => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "DNS upstream timed out",
            )),
        }
    }
    async fn forward(
        self: Arc<Self>,
        client: SocketAddr,
        bytes: &[u8],
        query: &Message,
        deadline: tokio::time::Instant,
    ) -> io::Result<Vec<u8>> {
        let domain = canonical(&query.query().unwrap().name().to_ascii());
        let mut decision = self.router.route_dns(&self.cfg.router, &domain).await?;
        if decision.matched_rule.is_none() {
            for parent in self
                .table
                .get_cname_parents(&self.cfg.router, client.ip(), &domain)
            {
                if let Ok(parent_decision) =
                    self.router.route_dns(&self.cfg.router, &parent).await
                {
                    if parent_decision.matched_rule.is_some() {
                        decision = parent_decision;
                        break;
                    }
                }
            }
        }
        let matched_cfg = self
            .config
            .connector
            .iter()
            .find(|c| c.name == decision.outbound)
            .ok_or_else(|| io::Error::other("DNS connector not found"))?;
        if matched_cfg.proto == "block" {
            return Ok(message::error_reply(query, ResponseCode::Refused));
        }
        if self.cfg.disable_aaaa
            && query
                .query()
                .is_some_and(|q| q.query_type() == RecordType::AAAA)
        {
            return Ok(message::error_reply(query, ResponseCode::NoError));
        }
        let (cfg, outbound_name, chosen) = if decision.matched_rule.is_some() {
            if let Some(custom_dns) = matched_cfg
                .dns
                .as_ref()
                .filter(|v| !v.is_empty() && *v != &self.cfg.default_servers)
            {
                (matched_cfg, decision.outbound.clone(), custom_dns)
            } else if matched_cfg.proto == "tcp" {
                let chosen = matched_cfg
                    .dns
                    .as_ref()
                    .filter(|v| !v.is_empty())
                    .unwrap_or(&self.cfg.default_servers);
                (matched_cfg, decision.outbound.clone(), chosen)
            } else {
                let default_outbound = self
                    .config
                    .router
                    .iter()
                    .find(|r| r.name == self.cfg.router)
                    .map(|r| r.default.as_str())
                    .unwrap_or(decision.outbound.as_str());
                let default_cfg = self
                    .config
                    .connector
                    .iter()
                    .find(|c| c.name == default_outbound)
                    .unwrap_or(matched_cfg);
                (
                    default_cfg,
                    default_cfg.name.clone(),
                    &self.cfg.default_servers,
                )
            }
        } else {
            (
                matched_cfg,
                decision.outbound.clone(),
                &self.cfg.default_servers,
            )
        };
        log::debug!(
            "DNS client={client} domain={domain} rule={:?} outbound={} dns_via={}",
            decision.matched_rule,
            decision.outbound,
            outbound_name
        );

        let now = Instant::now();
        let mut stale_entry: Option<CachedResponse> = None;
        if let Some(key) = Self::cache_key(query, &domain, &outbound_name) {
            let lookup = {
                let mut cache = self.response_cache.lock().unwrap();
                cache.get(&key).cloned()
            };
            if let Some(entry) = lookup {
                let elapsed = now.saturating_duration_since(entry.inserted_at);
                if elapsed < entry.fresh_ttl {
                    if elapsed.saturating_mul(4) >= entry.fresh_ttl.saturating_mul(3) {
                        self.maybe_spawn_prefetch(
                            key.clone(),
                            outbound_name.clone(),
                            cfg.clone(),
                            chosen.clone(),
                            client,
                            bytes.to_vec(),
                            query.clone(),
                        );
                    }
                    if let Some(wire) = Self::build_cached_reply(&entry, query, now, false) {
                        self.counters.cache_hits.fetch_add(1, Ordering::Relaxed);
                        self.learn_records(
                            client,
                            &domain,
                            &entry.cnames,
                            &entry.addresses,
                            now,
                        )
                        .await;
                        return Ok(wire);
                    }
                } else if now <= entry.stale_until {
                    stale_entry = Some(entry);
                }
            }

            let rx = self.spawn_singleflight_leader(
                key,
                outbound_name.clone(),
                cfg.clone(),
                chosen.clone(),
                client,
                bytes.to_vec(),
                query.clone(),
            );
            let wait_deadline = if stale_entry.is_some() {
                (tokio::time::Instant::now() + Duration::from_millis(STALE_REFRESH_GRACE_MS))
                    .min(deadline)
            } else {
                deadline
            };
            match self.wait_singleflight(rx, wait_deadline).await {
                Ok(InflightState::Cached(entry)) => {
                    let learn_now = Instant::now();
                    self.learn_records(
                        client,
                        &domain,
                        &entry.cnames,
                        &entry.addresses,
                        learn_now,
                    )
                    .await;
                    if let Some(wire) = Self::build_cached_reply(&entry, query, learn_now, false) {
                        return Ok(wire);
                    }
                }
                Ok(InflightState::Raw(raw_data)) => {
                    if let Ok(resp) = Message::from_vec(&raw_data) {
                        let cnames = message::cname_chain(query, &resp);
                        let addresses = message::addresses(query, &resp);
                        self.learn_records(client, &domain, &cnames, &addresses, Instant::now())
                            .await;
                    }
                    return Ok(Self::patch_raw_reply(raw_data, query));
                }
                Ok(InflightState::Pending | InflightState::Failed) | Err(_) => {
                    if let Some(entry) = stale_entry {
                        if let Some(wire) = Self::build_cached_reply(&entry, query, now, true) {
                            self.counters.stale_hits.fetch_add(1, Ordering::Relaxed);
                            self.learn_records(
                                client,
                                &domain,
                                &entry.cnames,
                                &entry.addresses,
                                now,
                            )
                            .await;
                            return Ok(wire);
                        }
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "all DNS upstreams failed",
                    ));
                }
            }
        }

        // Fallback path for queries carrying non-cacheable EDNS options (e.g. NSID / ECS)
        let (raw_data, response) = self
            .query_upstreams_hedged(&outbound_name, cfg, chosen, client, bytes, query, deadline)
            .await?;
        let cnames = message::cname_chain(query, &response);
        let addresses = message::addresses(query, &response);
        self.learn_records(client, &domain, &cnames, &addresses, Instant::now())
            .await;
        Ok(raw_data)
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
                    client_port: Some(client.local_addr().unwrap().port()),
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

    async fn mock_custom(
        ip: Arc<StdMutex<&'static str>>,
        ttl: u32,
        delay_ms: Arc<AtomicU64>,
        hits: Arc<AtomicU64>,
        blackhole: Arc<std::sync::atomic::AtomicBool>,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut buf = vec![0; 65535];
            loop {
                let (n, c) = socket.recv_from(&mut buf).await.unwrap();
                hits.fetch_add(1, Ordering::Relaxed);
                if blackhole.load(Ordering::Relaxed) {
                    continue;
                }
                let d = delay_ms.load(Ordering::Relaxed);
                if d > 0 {
                    tokio::time::sleep(Duration::from_millis(d)).await;
                }
                let q = Message::from_vec(&buf[..n]).unwrap();
                let cur_ip: std::net::Ipv4Addr = ip.lock().unwrap().parse().unwrap();
                let mut r = Message::new();
                r.set_id(q.id())
                    .set_message_type(MessageType::Response)
                    .set_response_code(ResponseCode::NoError)
                    .add_query(q.queries()[0].clone())
                    .add_answer(Record::from_rdata(
                        q.queries()[0].name().clone(),
                        ttl,
                        RData::A(A(cur_ip)),
                    ));
                socket.send_to(&r.to_vec().unwrap(), c).await.unwrap();
            }
        });
        (addr, task)
    }

    #[tokio::test]
    async fn hedged_upstream_avoids_slow_first_server() {
        let hits1 = Arc::new(AtomicU64::new(0));
        let hits2 = Arc::new(AtomicU64::new(0));
        let (s1, t1) = mock_custom(
            Arc::new(StdMutex::new("1.1.1.1")),
            60,
            Arc::new(AtomicU64::new(0)),
            hits1.clone(),
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        )
        .await;
        let (s2, t2) = mock_custom(
            Arc::new(StdMutex::new("2.2.2.2")),
            60,
            Arc::new(AtomicU64::new(5)),
            hits2.clone(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        let mut cfg = configuration(s1, s2);
        let settings = cfg.dns.as_mut().unwrap();
        settings.upstream_timeout_ms = 2000;
        settings.request_timeout_ms = 4000;
        settings.endpoint = "127.0.0.1:0".into();

        let router = Arc::new(
            DefaultRouter::new(&cfg.router, cfg.data.as_ref().unwrap(), Resolver::new()).await,
        );
        let server = DnsServer::bind(
            settings.clone(),
            cfg,
            router,
            Arc::new(IpDomainTable::new(100, 10, 100)),
        )
        .await
        .unwrap();
        let addr = server.socket.local_addr().unwrap();
        let token = CancellationToken::new();
        let task = tokio::spawn(server.run(token.clone()));

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let start = Instant::now();
        let resp = send(&client, addr, "hedged.example", 42).await;
        let elapsed = start.elapsed();

        assert_eq!(
            resp.answers()[0].data(),
            &RData::A(A("2.2.2.2".parse().unwrap()))
        );
        assert!(
            elapsed < Duration::from_millis(600),
            "hedged query should avoid full 2000ms timeout, took {elapsed:?}"
        );
        assert_eq!(hits1.load(Ordering::Relaxed), 1);
        assert_eq!(hits2.load(Ordering::Relaxed), 1);

        token.cancel();
        task.await.unwrap().unwrap();
        t1.abort();
        t2.abort();
    }

    #[tokio::test]
    async fn response_cache_isolation_and_singleflight_coalescing() {
        let hits = Arc::new(AtomicU64::new(0));
        let (s1, t1) = mock_custom(
            Arc::new(StdMutex::new("9.9.9.9")),
            60,
            Arc::new(AtomicU64::new(60)),
            hits.clone(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        let mut cfg = configuration(s1, s1);
        let settings = cfg.dns.as_mut().unwrap();
        settings.upstream_timeout_ms = 2000;
        settings.request_timeout_ms = 4000;
        settings.endpoint = "127.0.0.1:0".into();

        let table = Arc::new(IpDomainTable::new(100, 10, 100));
        let router = Arc::new(
            DefaultRouter::new(&cfg.router, cfg.data.as_ref().unwrap(), Resolver::new()).await,
        );
        let server = DnsServer::bind(settings.clone(), cfg, router, table.clone())
            .await
            .unwrap();
        let addr = server.socket.local_addr().unwrap();
        let token = CancellationToken::new();
        let task = tokio::spawn(server.clone().run(token.clone()));

        let c1 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let c2 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let c3 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (r1, r2, r3) = tokio::join!(
            send(&c1, addr, "sf.example", 101),
            send(&c2, addr, "sf.example", 102),
            send(&c3, addr, "sf.example", 103)
        );
        assert_eq!(r1.id(), 101);
        assert_eq!(r2.id(), 102);
        assert_eq!(r3.id(), 103);
        assert_eq!(hits.load(Ordering::Relaxed), 1);

        // Wait a beat and query again from a 4th client socket -> cache hit (still 1 upstream hit!)
        let c4 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let r4 = send(&c4, addr, "sf.example", 104).await;
        assert_eq!(r4.id(), 104);
        assert_eq!(hits.load(Ordering::Relaxed), 1);
        assert_eq!(
            table.get_domains("main", "127.0.0.1".parse().unwrap(), "9.9.9.9".parse().unwrap()),
            vec!["sf.example"]
        );

        token.cancel();
        task.await.unwrap().unwrap();
        t1.abort();
    }

    #[tokio::test]
    async fn stale_grace_fallback_and_background_refresh() {
        let ip_ref = Arc::new(StdMutex::new("10.10.10.1"));
        let delay_ms = Arc::new(AtomicU64::new(0));
        let hits = Arc::new(AtomicU64::new(0));
        let (s1, t1) = mock_custom(
            ip_ref.clone(),
            1,
            delay_ms.clone(),
            hits.clone(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .await;
        let mut cfg = configuration(s1, s1);
        let settings = cfg.dns.as_mut().unwrap();
        settings.upstream_timeout_ms = 2000;
        settings.request_timeout_ms = 4000;
        settings.endpoint = "127.0.0.1:0".into();

        let router = Arc::new(
            DefaultRouter::new(&cfg.router, cfg.data.as_ref().unwrap(), Resolver::new()).await,
        );
        let server = DnsServer::bind(
            settings.clone(),
            cfg,
            router,
            Arc::new(IpDomainTable::new(100, 10, 100)),
        )
        .await
        .unwrap();
        let addr = server.socket.local_addr().unwrap();
        let token = CancellationToken::new();
        let task = tokio::spawn(server.clone().run(token.clone()));

        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let first = send(&client, addr, "stale.example", 201).await;
        assert_eq!(
            first.answers()[0].data(),
            &RData::A(A("10.10.10.1".parse().unwrap()))
        );

        // Let the 1s TTL expire, then make upstream take 450ms (> 250ms stale grace) with new IP
        tokio::time::sleep(Duration::from_millis(1100)).await;
        *ip_ref.lock().unwrap() = "10.10.10.2";
        delay_ms.store(450, Ordering::Relaxed);

        let start = Instant::now();
        let second = send(&client, addr, "stale.example", 202).await;
        let elapsed = start.elapsed();
        assert_eq!(
            second.answers()[0].data(),
            &RData::A(A("10.10.10.1".parse().unwrap()))
        );
        assert!(
            elapsed < Duration::from_millis(400),
            "stale grace should return within ~250ms, took {elapsed:?}"
        );
        assert_eq!(server.counters.stale_hits.load(Ordering::Relaxed), 1);

        // Wait for the background singleflight leader to finish refreshing cache with 10.10.10.2
        tokio::time::sleep(Duration::from_millis(350)).await;
        let third = send(&client, addr, "stale.example", 203).await;
        assert_eq!(
            third.answers()[0].data(),
            &RData::A(A("10.10.10.2".parse().unwrap()))
        );

        token.cancel();
        task.await.unwrap().unwrap();
        t1.abort();
    }
}

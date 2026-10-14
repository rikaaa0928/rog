use crate::def::{RunAccStream, RunAcceptor, RunStream};
use crate::object::config::ObjectConfig;
use crate::util::RunAddr;
#[allow(unused_imports)]
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[cfg(target_os = "linux")]
use crate::def::{RunUdpReader, RunUdpWriter, UDPMeta, UDPPacket};
#[cfg(target_os = "linux")]
use crate::stream::tcp::TcpRunStream;
#[cfg(target_os = "linux")]
use log::{debug, error, info, warn};
#[cfg(target_os = "linux")]
use lru::LruCache;
#[cfg(target_os = "linux")]
use socket2::{Domain, Protocol, Socket, Type};
#[cfg(target_os = "linux")]
use std::mem;
#[cfg(target_os = "linux")]
use std::num::NonZeroUsize;
#[cfg(target_os = "linux")]
use std::os::unix::io::AsRawFd;
#[cfg(target_os = "linux")]
use std::time::Instant;
#[cfg(target_os = "linux")]
use tokio::io::unix::AsyncFd;

#[allow(dead_code)]
const DEFAULT_TPROXY_SENDER_LRU_CAP: usize = 512;
#[allow(dead_code)]
const DEFAULT_UDP_IDLE_TIMEOUT_SECS: u64 = 60;
#[allow(dead_code)]
const TPROXY_UDP_RECV_BUF_SIZE: usize = 65536; // 64 KiB to safely hold maximum UDP datagram size

#[cfg(target_os = "linux")]
const IP_TRANSPARENT: libc::c_int = 19;
#[cfg(target_os = "linux")]
const IP_RECVORIGDSTADDR: libc::c_int = 20;
#[cfg(target_os = "linux")]
const IP_ORIGDSTADDR: libc::c_int = 20;
#[cfg(target_os = "linux")]
const IPV6_RECVORIGDSTADDR: libc::c_int = 74;
#[cfg(target_os = "linux")]
const IPV6_ORIGDSTADDR: libc::c_int = 74;
#[cfg(target_os = "linux")]
const IPV6_TRANSPARENT: libc::c_int = 75;

#[allow(dead_code)]
#[repr(C, align(8))]
struct AlignedControlBuf([u8; 256]);

/// Converts an IPv4-mapped IPv6 address (e.g. ::ffff:192.168.1.1) to canonical IPv4.
/// Leaves standard IPv6 (including loopback ::1) intact without misconverting to 0.0.0.1.
#[allow(dead_code)]
pub fn unmapped_socket_addr(addr: SocketAddr) -> SocketAddr {
    match addr {
        SocketAddr::V6(v6) => {
            if let Some(v4) = v6.ip().to_ipv4_mapped() {
                SocketAddr::new(std::net::IpAddr::V4(v4), v6.port())
            } else {
                SocketAddr::V6(v6)
            }
        }
        v4 => v4,
    }
}

/// Safely parse an address string and port into a SocketAddr, handling both IPv4
/// and IPv6 addresses (with or without brackets) without panicking.
#[allow(dead_code)]
pub fn parse_endpoint(addr: &str, port: u16) -> Option<SocketAddr> {
    crate::util::format_endpoint(addr, port)
        .parse::<SocketAddr>()
        .ok()
}

/// Check if an IP address belongs to any local network interface on this machine.
#[cfg(unix)]
#[allow(dead_code)]
pub fn is_local_host_ip(ip: &std::net::IpAddr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    unsafe {
        let mut ifaddrs: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifaddrs) != 0 || ifaddrs.is_null() {
            return false;
        }
        let mut cur = ifaddrs;
        let mut found = false;
        while !cur.is_null() {
            let addr = (*cur).ifa_addr;
            if !addr.is_null() {
                let family = (*addr).sa_family as libc::c_int;
                if family == libc::AF_INET {
                    let sin = &*(addr as *const libc::sockaddr_in);
                    let if_ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                    if std::net::IpAddr::V4(if_ip) == *ip {
                        found = true;
                        break;
                    }
                } else if family == libc::AF_INET6 {
                    let sin6 = &*(addr as *const libc::sockaddr_in6);
                    let if_ip = std::net::Ipv6Addr::from(sin6.sin6_addr.s6_addr);
                    if std::net::IpAddr::V6(if_ip) == *ip {
                        found = true;
                        break;
                    }
                }
            }
            cur = (*cur).ifa_next;
        }
        libc::freeifaddrs(ifaddrs);
        found
    }
}

#[cfg(not(unix))]
#[allow(dead_code)]
pub fn is_local_host_ip(ip: &std::net::IpAddr) -> bool {
    ip.is_loopback()
}

/// Detects if an intercepted connection or datagram is actually directed to rog's own listening port,
/// including wildcards (0.0.0.0 / [::]) with 127.0.0.1 / [::1] or any local network interface address.
#[allow(dead_code)]
pub fn is_self_connection(target: &SocketAddr, listen_addr: &SocketAddr) -> bool {
    let target = unmapped_socket_addr(*target);
    let listen = unmapped_socket_addr(*listen_addr);

    // Fast path: if the port doesn't match the listener's port, it's not a self-connection
    if target.port() != listen.port() {
        return false;
    }

    // Exact address and port match
    if target == listen {
        return true;
    }

    // Target is unspecified (0.0.0.0 or [::])
    if target.ip().is_unspecified() {
        return true;
    }

    // If the listener is on a wildcard (0.0.0.0 or [::])
    if listen.ip().is_unspecified() {
        if target.ip().is_loopback() {
            return true;
        }
        if is_local_host_ip(&target.ip()) {
            return true;
        }
    }

    false
}

#[allow(dead_code)]
pub struct TproxyOptions {
    pub mode: String, // "tcp+udp", "tcp", "udp"
    pub udp_idle_timeout_secs: u64,
    pub udp_max_senders: usize,
}

impl TproxyOptions {
    #[allow(dead_code)]
    pub fn from_listener(listener: &crate::def::config::Listener) -> Self {
        let mut mode = "tcp+udp".to_string();
        let mut udp_idle_timeout_secs = DEFAULT_UDP_IDLE_TIMEOUT_SECS;
        let mut udp_max_senders = DEFAULT_TPROXY_SENDER_LRU_CAP;

        if let Some(ref opts) = listener.options {
            if let Some(m) = opts.get("mode").and_then(|v| v.as_str()) {
                mode = m.to_lowercase();
            }
            if let Some(timeout) = opts.get("udp_idle_timeout_secs").and_then(|v| v.as_integer()) {
                if timeout > 0 {
                    udp_idle_timeout_secs = timeout as u64;
                }
            } else if let Some(timeout) = opts.get("udp_timeout").and_then(|v| v.as_integer()) {
                if timeout > 0 {
                    udp_idle_timeout_secs = timeout as u64;
                }
            }
            if let Some(max_senders) = opts.get("udp_max_senders").and_then(|v| v.as_integer()) {
                if max_senders > 0 {
                    udp_max_senders = max_senders as usize;
                }
            }
        }

        Self {
            mode,
            udp_idle_timeout_secs,
            udp_max_senders,
        }
    }
}

#[allow(dead_code)]
pub struct TproxyRunAcceptor {
    rx: Arc<Mutex<tokio::sync::mpsc::Receiver<(RunAccStream, SocketAddr)>>>,
}

#[async_trait::async_trait]
impl RunAcceptor for TproxyRunAcceptor {
    async fn accept(&self) -> io::Result<(RunAccStream, SocketAddr)> {
        let mut rx = self.rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "tproxy listener closed"))
    }

    async fn handshake(
        &self,
        stream: &mut dyn RunStream,
    ) -> io::Result<(RunAddr, Option<Vec<u8>>)> {
        let info = stream.get_info();
        let dst_addr = info.dst_addr.clone().unwrap_or_default();
        let dst_port = info.dst_port.unwrap_or(0);
        let udp = info.udp.unwrap_or(false);

        Ok((
            RunAddr {
                addr: dst_addr,
                port: dst_port,
                udp,
            },
            None,
        ))
    }

    async fn post_handshake(
        &self,
        _stream: &mut dyn RunStream,
        _error: bool,
        _port: u16,
    ) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn setsockopt_int(
    fd: libc::c_int,
    level: libc::c_int,
    optname: libc::c_int,
    val: libc::c_int,
) -> io::Result<()> {
    let ret = unsafe {
        libc::setsockopt(
            fd,
            level,
            optname,
            &val as *const libc::c_int as *const libc::c_void,
            mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if ret != 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn set_ip_transparent(fd: libc::c_int, is_v6: bool) -> io::Result<()> {
    if is_v6 {
        setsockopt_int(fd, libc::SOL_IPV6, IPV6_TRANSPARENT, 1)?;
        // some kernels also need the v4 option on a dual-stack socket
        let _ = setsockopt_int(fd, libc::SOL_IP, IP_TRANSPARENT, 1);
        Ok(())
    } else {
        setsockopt_int(fd, libc::SOL_IP, IP_TRANSPARENT, 1)
    }
}

#[cfg(target_os = "linux")]
fn set_recv_origdstaddr(fd: libc::c_int, is_v6: bool) -> io::Result<()> {
    if is_v6 {
        setsockopt_int(fd, libc::SOL_IPV6, IPV6_RECVORIGDSTADDR, 1)?;
        let _ = setsockopt_int(fd, libc::SOL_IP, IP_RECVORIGDSTADDR, 1);
        Ok(())
    } else {
        setsockopt_int(fd, libc::SOL_IP, IP_RECVORIGDSTADDR, 1)
    }
}

#[cfg(unix)]
#[allow(dead_code)]
fn storage_to_socketaddr(
    storage: &libc::sockaddr_storage,
    len: libc::socklen_t,
) -> Option<SocketAddr> {
    let mut sock_storage = socket2::SockAddrStorage::zeroed();
    let dest = unsafe { sock_storage.view_as::<libc::sockaddr_storage>() };
    *dest = *storage;
    let sa = unsafe { socket2::SockAddr::new(sock_storage, len) };
    sa.as_socket()
}

#[cfg(target_os = "linux")]
fn new_tproxy_tcp_listener(addr: SocketAddr) -> io::Result<tokio::net::TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    if addr.is_ipv6() {
        let _ = socket.set_only_v6(false);
    }
    set_ip_transparent(socket.as_raw_fd(), addr.is_ipv6())?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    let std_listener: std::net::TcpListener = socket.into();
    tokio::net::TcpListener::from_std(std_listener)
}

#[cfg(target_os = "linux")]
fn new_tproxy_udp_listener(addr: SocketAddr) -> io::Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    if addr.is_ipv6() {
        let _ = socket.set_only_v6(false);
    }
    set_ip_transparent(socket.as_raw_fd(), addr.is_ipv6())?;
    set_recv_origdstaddr(socket.as_raw_fd(), addr.is_ipv6())?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    Ok(socket.into())
}

#[cfg(target_os = "linux")]
fn new_tproxy_udp_sender(src: SocketAddr) -> io::Result<tokio::net::UdpSocket> {
    let socket = Socket::new(Domain::for_address(src), Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    set_ip_transparent(socket.as_raw_fd(), src.is_ipv6())?;
    socket.set_nonblocking(true)?;
    socket.bind(&src.into())?;
    let std_socket: std::net::UdpSocket = socket.into();
    tokio::net::UdpSocket::from_std(std_socket)
}

#[cfg(target_os = "linux")]
fn recv_with_origdst(
    fd: libc::c_int,
    buf: &mut [u8],
) -> io::Result<(usize, SocketAddr, SocketAddr)> {
    unsafe {
        let mut name: libc::sockaddr_storage = mem::zeroed();
        let mut iov = libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut libc::c_void,
            iov_len: buf.len(),
        };
        let mut control = AlignedControlBuf([0u8; 256]);
        let mut msg: libc::msghdr = mem::zeroed();
        msg.msg_name = &mut name as *mut libc::sockaddr_storage as *mut libc::c_void;
        msg.msg_namelen = mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.0.as_mut_ptr() as *mut libc::c_void;
        msg.msg_controllen = control.0.len() as _;

        let n = libc::recvmsg(fd, &mut msg, 0);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }

        if (msg.msg_flags & libc::MSG_TRUNC) != 0 {
            return Err(io::Error::other(format!(
                "tproxy: udp datagram truncated (reported size {}, buffer was {})",
                n,
                buf.len()
            )));
        }

        let raw_src = storage_to_socketaddr(&name, msg.msg_namelen)
            .ok_or_else(|| io::Error::other("tproxy: bad source address"))?;
        let src = unmapped_socket_addr(raw_src);

        let mut dst: Option<SocketAddr> = None;
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            let level = (*cmsg).cmsg_level;
            let ctype = (*cmsg).cmsg_type;
            let is_v4 = level == libc::SOL_IP && ctype == IP_ORIGDSTADDR;
            let is_v6 = level == libc::SOL_IPV6 && ctype == IPV6_ORIGDSTADDR;
            if is_v4 || is_v6 {
                let required_len = if is_v4 {
                    mem::size_of::<libc::sockaddr_in>()
                } else {
                    mem::size_of::<libc::sockaddr_in6>()
                };
                let cmsg_len = (*cmsg).cmsg_len as usize;
                let header_len = libc::CMSG_LEN(0) as usize;
                if cmsg_len >= header_len + required_len {
                    let data = libc::CMSG_DATA(cmsg) as *const u8;
                    let mut storage: libc::sockaddr_storage = mem::zeroed();
                    std::ptr::copy_nonoverlapping(
                        data,
                        &mut storage as *mut libc::sockaddr_storage as *mut u8,
                        required_len,
                    );
                    if let Some(raw_dst) =
                        storage_to_socketaddr(&storage, required_len as libc::socklen_t)
                    {
                        dst = Some(unmapped_socket_addr(raw_dst));
                    }
                } else {
                    warn!(
                        "tproxy: origdst cmsg too short: {} < {}",
                        cmsg_len,
                        header_len + required_len
                    );
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }

        let dst =
            dst.ok_or_else(|| io::Error::other("tproxy: missing original destination cmsg"))?;
        Ok((n as usize, src, dst))
    }
}

#[cfg(target_os = "linux")]
struct TproxySendHalf {
    senders: LruCache<SocketAddr, tokio::net::UdpSocket>,
}

#[cfg(target_os = "linux")]
impl TproxySendHalf {
    fn new(cap: usize) -> Self {
        let cap = NonZeroUsize::new(cap)
            .unwrap_or_else(|| NonZeroUsize::new(DEFAULT_TPROXY_SENDER_LRU_CAP).unwrap());
        Self {
            senders: LruCache::new(cap),
        }
    }

    async fn send_to(
        &mut self,
        buf: &[u8],
        src_addr: &SocketAddr,
        dst_addr: &SocketAddr,
    ) -> io::Result<usize> {
        let src = unmapped_socket_addr(*src_addr);
        let mut dst = unmapped_socket_addr(*dst_addr);

        // Defensive normalization: if one is IPv4 and one is IPv6, align them
        if src.is_ipv4() && dst.is_ipv6() {
            if let std::net::IpAddr::V6(v6) = dst.ip() {
                if let Some(v4) = v6.to_ipv4_mapped() {
                    dst = SocketAddr::new(std::net::IpAddr::V4(v4), dst.port());
                }
            }
        } else if src.is_ipv6() && dst.is_ipv4() {
            if let std::net::IpAddr::V4(v4) = dst.ip() {
                dst = SocketAddr::new(std::net::IpAddr::V6(v4.to_ipv6_mapped()), dst.port());
            }
        }

        if !self.senders.contains(&src) {
            let socket = new_tproxy_udp_sender(src)?;
            self.senders.put(src, socket);
        }
        let socket = self
            .senders
            .get_mut(&src)
            .expect("sender socket must exist after ensure");
        socket.send_to(buf, &dst).await
    }
}

#[cfg(target_os = "linux")]
struct TproxySessionReader {
    rx: tokio::sync::mpsc::Receiver<UDPPacket>,
}

#[cfg(target_os = "linux")]
#[async_trait::async_trait]
impl RunUdpReader for TproxySessionReader {
    async fn read(&mut self) -> io::Result<UDPPacket> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::ConnectionReset, "udp session closed"))
    }
}

#[cfg(target_os = "linux")]
struct TproxySessionWriter {
    sender: Arc<Mutex<TproxySendHalf>>,
    client_addr: SocketAddr,
    orig_dst: SocketAddr,
}

#[cfg(target_os = "linux")]
#[async_trait::async_trait]
impl RunUdpWriter for TproxySessionWriter {
    async fn write(&self, packet: UDPPacket) -> io::Result<()> {
        // According to rog's UDP packet contract (see udp.rs line 132):
        // packet.meta.dst_addr / dst_port is the remote target server.
        // packet.meta.src_addr / src_port is the original client.
        // For transparent proxy replies, we must spoof the source as the remote target,
        // and send the datagram to the client destination!
        let spoofed_src: SocketAddr = parse_endpoint(&packet.meta.dst_addr, packet.meta.dst_port)
            .unwrap_or(self.orig_dst);
        let client_dst: SocketAddr = parse_endpoint(&packet.meta.src_addr, packet.meta.src_port)
            .unwrap_or(self.client_addr);

        let mut sender_guard = self.sender.lock().await;
        sender_guard
            .send_to(&packet.data, &spoofed_src, &client_dst)
            .await
            .map(|_| ())
    }
}

#[cfg(target_os = "linux")]
struct SessionEntry {
    tx: tokio::sync::mpsc::Sender<UDPPacket>,
    last_activity: Instant,
}

pub struct TproxyListener {
    cfg: ObjectConfig,
    shutdown: CancellationToken,
}

impl TproxyListener {
    pub fn new(cfg: ObjectConfig, shutdown: CancellationToken) -> Self {
        Self { cfg, shutdown }
    }

    #[cfg(target_os = "linux")]
    pub async fn listen(&self) -> io::Result<Box<dyn RunAcceptor>> {
        let endpoint_str = self.cfg.listener.endpoint.trim();
        let listen_addr: SocketAddr = endpoint_str.parse().map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid endpoint {}: {}", endpoint_str, e),
            )
        })?;

        let options = TproxyOptions::from_listener(&self.cfg.listener);
        let (accept_tx, accept_rx) =
            tokio::sync::mpsc::channel::<(RunAccStream, SocketAddr)>(1024);

        let run_tcp = options.mode.contains("tcp");
        let run_udp = options.mode.contains("udp");

        if !run_tcp && !run_udp {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid tproxy mode '{}': must contain tcp or udp", options.mode),
            ));
        }

        // 1. TCP Listener Runner
        if run_tcp {
            let tcp_listener = new_tproxy_tcp_listener(listen_addr)?;
            info!("tproxy listening tcp {}", listen_addr);
            let accept_tx_tcp = accept_tx.clone();
            let shutdown_tcp = self.shutdown.clone();

            tokio::spawn(async move {
                let unmapped_listen = unmapped_socket_addr(listen_addr);
                loop {
                    tokio::select! {
                        _ = shutdown_tcp.cancelled() => {
                            debug!("tproxy tcp listener cancelled");
                            break;
                        }
                        res = tcp_listener.accept() => {
                            match res {
                                Ok((stream, peer)) => {
                                    let orig = match stream.local_addr() {
                                        Ok(a) => unmapped_socket_addr(a),
                                        Err(e) => {
                                            debug!("tproxy: no original destination: {}", e);
                                            continue;
                                        }
                                    };
                                    let peer = unmapped_socket_addr(peer);
                                    if is_self_connection(&orig, &listen_addr) {
                                        debug!("tproxy: loopback TCP connection detected to {}, dropping", orig);
                                        continue;
                                    }
                                    let _ = stream.set_nodelay(true);
                                    let mut tcp_stream = TcpRunStream::new(stream);
                                    tcp_stream.set_info(&mut |x| {
                                        x.protocol_name = "tproxy".to_string();
                                        x.src_addr = Some(peer.ip().to_string());
                                        x.src_port = Some(peer.port());
                                        x.dst_addr = Some(orig.ip().to_string());
                                        x.dst_port = Some(orig.port());
                                        x.udp = Some(false);
                                    });
                                    if accept_tx_tcp.send((RunAccStream::TCPStream(Box::new(tcp_stream)), peer)).await.is_err() {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    debug!("tproxy tcp accept error: {}", e);
                                }
                            }
                        }
                    }
                }
            });
        }

        // 2. UDP Listener Runner
        if run_udp {
            let udp_socket = new_tproxy_udp_listener(listen_addr)?;
            let async_fd = AsyncFd::new(udp_socket)?;
            info!("tproxy listening udp {}", listen_addr);

            let accept_tx_udp = accept_tx.clone();
            let shutdown_udp = self.shutdown.clone();
            let shutdown_reaper = self.shutdown.clone();
            let send_half = Arc::new(Mutex::new(TproxySendHalf::new(options.udp_max_senders)));
            let sessions: Arc<Mutex<HashMap<(SocketAddr, SocketAddr), SessionEntry>>> =
                Arc::new(Mutex::new(HashMap::new()));
            let sessions_recv = sessions.clone();
            let sessions_reaper = sessions.clone();
            let idle_timeout = tokio::time::Duration::from_secs(options.udp_idle_timeout_secs);

            // UDP receiver loop
            tokio::spawn(async move {
                let mut buf = vec![0u8; TPROXY_UDP_RECV_BUF_SIZE];
                let unmapped_listen = unmapped_socket_addr(listen_addr);
                loop {
                    let recv_res = tokio::select! {
                        _ = shutdown_udp.cancelled() => {
                            debug!("tproxy udp receiver cancelled");
                            break;
                        }
                        guard_res = async_fd.readable() => {
                            match guard_res {
                                Ok(mut guard) => {
                                    match guard.try_io(|inner| recv_with_origdst(inner.get_ref().as_raw_fd(), &mut buf)) {
                                        Ok(res) => res,
                                        Err(_would_block) => continue,
                                    }
                                }
                                Err(e) => Err(e),
                            }
                        }
                    };

                    match recv_res {
                        Ok((n, src_addr, orig_dst)) => {
                            let src_addr = unmapped_socket_addr(src_addr);
                            let orig_dst = unmapped_socket_addr(orig_dst);
                            if is_self_connection(&orig_dst, &listen_addr) {
                                debug!("tproxy: loopback UDP datagram detected to {}, dropping", orig_dst);
                                continue;
                            }

                            let mut pkt = UDPPacket {
                                meta: UDPMeta {
                                    dst_addr: orig_dst.ip().to_string(),
                                    dst_port: orig_dst.port(),
                                    src_addr: src_addr.ip().to_string(),
                                    src_port: src_addr.port(),
                                },
                                data: buf[..n].to_vec(),
                            };

                            let key = (src_addr, orig_dst);
                            let mut sessions_guard = sessions_recv.lock().await;

                            if let Some(entry) = sessions_guard.get_mut(&key) {
                                if !entry.tx.is_closed() {
                                    match entry.tx.try_send(pkt) {
                                        Ok(()) => {
                                            entry.last_activity = Instant::now();
                                            continue;
                                        }
                                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                            // Channel buffer is full; drop packet under backpressure but keep session active
                                            entry.last_activity = Instant::now();
                                            continue;
                                        }
                                        Err(tokio::sync::mpsc::error::TrySendError::Closed(p)) => {
                                            // Receiver was dropped/closed; remove dead session and recreate below
                                            pkt = p;
                                            sessions_guard.remove(&key);
                                        }
                                    }
                                } else {
                                    sessions_guard.remove(&key);
                                }
                            }

                            // At this point, either the session was not present, or the dead session was removed
                            // and pkt was reclaimed. pkt is guaranteed to be owned and valid here.
                            let (tx, rx) = tokio::sync::mpsc::channel(128);
                            let _ = tx.try_send(pkt);
                            sessions_guard.insert(
                                key,
                                SessionEntry {
                                    tx,
                                    last_activity: Instant::now(),
                                },
                            );
                            drop(sessions_guard);

                            let reader = TproxySessionReader { rx };
                            let writer = TproxySessionWriter {
                                sender: send_half.clone(),
                                client_addr: src_addr,
                                orig_dst,
                            };
                            if accept_tx_udp
                                .send((
                                    RunAccStream::UDPSocket((
                                        Box::new(reader),
                                        Box::new(writer),
                                    )),
                                    src_addr,
                                ))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            warn!("tproxy udp recv error: {}, retrying after backoff...", e);
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            });

            // UDP session reaper loop
            tokio::spawn(async move {
                let interval_duration = tokio::time::Duration::from_secs(5);
                loop {
                    tokio::select! {
                        _ = shutdown_reaper.cancelled() => break,
                        _ = tokio::time::sleep(interval_duration) => {
                            let mut guard = sessions_reaper.lock().await;
                            guard.retain(|_, entry| entry.last_activity.elapsed() < idle_timeout);
                        }
                    }
                }
            });
        }

        Ok(Box::new(TproxyRunAcceptor {
            rx: Arc::new(Mutex::new(accept_rx)),
        }))
    }

    #[cfg(not(target_os = "linux"))]
    pub async fn listen(&self) -> io::Result<Box<dyn RunAcceptor>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "tproxy inbound is only supported on Linux",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_unmapped_socket_addr() {
        let v6_loopback: SocketAddr = "[::1]:12345".parse().unwrap();
        assert_eq!(unmapped_socket_addr(v6_loopback), v6_loopback);

        let standard_v6: SocketAddr = "[240e:305:1890::1]:80".parse().unwrap();
        assert_eq!(unmapped_socket_addr(standard_v6), standard_v6);

        let mapped: SocketAddr = "[::ffff:192.168.0.71]:12345".parse().unwrap();
        let expected: SocketAddr = "192.168.0.71:12345".parse().unwrap();
        assert_eq!(unmapped_socket_addr(mapped), expected);

        let ipv4: SocketAddr = "192.168.1.1:80".parse().unwrap();
        assert_eq!(unmapped_socket_addr(ipv4), ipv4);
    }

    #[test]
    fn test_parse_endpoint() {
        assert_eq!(
            parse_endpoint("192.168.1.1", 80),
            Some("192.168.1.1:80".parse().unwrap())
        );
        assert_eq!(
            parse_endpoint("2001:db8::1", 53),
            Some("[2001:db8::1]:53".parse().unwrap())
        );
        assert_eq!(
            parse_endpoint("[2001:db8::1]", 53),
            Some("[2001:db8::1]:53".parse().unwrap())
        );
        assert_eq!(parse_endpoint("invalid ip", 80), None);
    }

    #[tokio::test]
    async fn test_try_send_closed_reclaim() {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(1);
        drop(rx); // simulate closed receiver

        let mut item = "test_payload".to_string();
        let need_new;
        if tx.is_closed() {
            need_new = true;
        } else {
            match tx.try_send(item) {
                Ok(()) => panic!("should not succeed"),
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => panic!("should not be full"),
                Err(tokio::sync::mpsc::error::TrySendError::Closed(p)) => {
                    item = p;
                    need_new = true;
                }
            }
        }
        assert!(need_new);

        let (tx2, mut rx2) = tokio::sync::mpsc::channel::<String>(1);
        tx2.try_send(item).unwrap();
        assert_eq!(rx2.recv().await.unwrap(), "test_payload");
    }

    #[test]
    fn test_is_self_connection() {
        let listen_any: SocketAddr = "0.0.0.0:12345".parse().unwrap();
        let listen_any_v6: SocketAddr = "[::]:12345".parse().unwrap();
        let listen_specific: SocketAddr = "192.168.1.10:12345".parse().unwrap();

        // 1. Different port -> false
        assert!(!is_self_connection(
            &"127.0.0.1:80".parse().unwrap(),
            &listen_any
        ));
        assert!(!is_self_connection(
            &"0.0.0.0:80".parse().unwrap(),
            &listen_any
        ));

        // 2. Exact match -> true
        assert!(is_self_connection(
            &"192.168.1.10:12345".parse().unwrap(),
            &listen_specific
        ));

        // 3. Loopback to wildcard listener -> true
        assert!(is_self_connection(
            &"127.0.0.1:12345".parse().unwrap(),
            &listen_any
        ));
        assert!(is_self_connection(
            &"127.0.0.2:12345".parse().unwrap(),
            &listen_any
        ));
        assert!(is_self_connection(
            &"[::1]:12345".parse().unwrap(),
            &listen_any_v6
        ));
        assert!(is_self_connection(
            &"[::ffff:127.0.0.1]:12345".parse().unwrap(),
            &listen_any_v6
        ));

        // 4. Remote IP to wildcard listener -> false
        assert!(!is_self_connection(
            &"8.8.8.8:12345".parse().unwrap(),
            &listen_any
        ));
        assert!(!is_self_connection(
            &"[2001:4860:4860::8888]:12345".parse().unwrap(),
            &listen_any_v6
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_storage_to_socketaddr() {
        let sa: SocketAddr = "192.168.1.1:80".parse().unwrap();
        let sock2_sa = socket2::SockAddr::from(sa);
        let mut libc_storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let len = sock2_sa.len();
        unsafe {
            std::ptr::copy_nonoverlapping(
                sock2_sa.as_ptr() as *const u8,
                &mut libc_storage as *mut libc::sockaddr_storage as *mut u8,
                len as usize,
            );
        }
        let converted = storage_to_socketaddr(&libc_storage, len).unwrap();
        assert_eq!(converted, sa);
    }

    #[test]
    fn test_tproxy_options_parsing() {
        let mut listener = crate::def::config::Listener {
            endpoint: "0.0.0.0:12345".to_string(),
            name: "tproxy_in".to_string(),
            user: None,
            pw: None,
            proto: "tproxy".to_string(),
            router: "default".to_string(),
            up: None,
            down: None,
            options: None,
        };

        let opts = TproxyOptions::from_listener(&listener);
        assert_eq!(opts.mode, "tcp+udp");
        assert_eq!(opts.udp_idle_timeout_secs, DEFAULT_UDP_IDLE_TIMEOUT_SECS);
        assert_eq!(opts.udp_max_senders, DEFAULT_TPROXY_SENDER_LRU_CAP);

        let mut map = HashMap::new();
        map.insert("mode".to_string(), toml::Value::String("tcp".to_string()));
        map.insert(
            "udp_idle_timeout_secs".to_string(),
            toml::Value::Integer(120),
        );
        map.insert("udp_max_senders".to_string(), toml::Value::Integer(1024));
        listener.options = Some(map);

        let opts = TproxyOptions::from_listener(&listener);
        assert_eq!(opts.mode, "tcp");
        assert_eq!(opts.udp_idle_timeout_secs, 120);
        assert_eq!(opts.udp_max_senders, 1024);
    }
}

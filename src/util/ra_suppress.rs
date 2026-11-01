use std::net::IpAddr;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[allow(dead_code)]
pub(crate) fn build_zero_lifetime_ra_payload(
    raw_icmp: &[u8],
    fallback_mac: Option<[u8; 6]>,
) -> Option<([u8; 6], Vec<u8>)> {
    if raw_icmp.len() < 16 || raw_icmp[0] != 134 {
        return None;
    }
    let cur_hop = raw_icmp[4];
    let router_lifetime = u16::from_be_bytes([raw_icmp[6], raw_icmp[7]]);
    let mut has_nonzero = router_lifetime > 0;
    let mut extracted_mac: Option<[u8; 6]> = None;
    let mut out_opts = Vec::new();

    let mut pos = 16;
    while pos + 2 <= raw_icmp.len() {
        let otype = raw_icmp[pos];
        let olen = (raw_icmp[pos + 1] as usize) * 8;
        if olen == 0 || pos + olen > raw_icmp.len() {
            break;
        }
        let mut opt = raw_icmp[pos..pos + olen].to_vec();
        match otype {
            1 if olen >= 8 => {
                let mut m = [0u8; 6];
                m.copy_from_slice(&opt[2..8]);
                if m != [0u8; 6] {
                    extracted_mac = Some(m);
                }
                out_opts.extend_from_slice(&opt);
            }
            3 if olen >= 32 => {
                // Prefix Information: Valid Lifetime [4..8], Preferred Lifetime [8..12]
                if opt[4..12].iter().any(|&b| b != 0) {
                    has_nonzero = true;
                }
                opt[4..12].fill(0);
                out_opts.extend_from_slice(&opt);
            }
            24 | 25 | 31 if olen >= 8 => {
                // Route Info (24), RDNSS (25), DNSSL (31): Lifetime [4..8]
                if opt[4..8].iter().any(|&b| b != 0) {
                    has_nonzero = true;
                }
                opt[4..8].fill(0);
                out_opts.extend_from_slice(&opt);
            }
            _ => {}
        }
        pos += olen;
    }

    if !has_nonzero {
        return None;
    }

    let router_mac = extracted_mac.or(fallback_mac)?;
    let mut payload = Vec::with_capacity(16 + 8 + out_opts.len());
    // ICMPv6 RA header: Type=134, Code=0, Checksum=0, CurHop, Flags=0, RouterLifetime=0, Reachable=0, Retrans=0
    payload.extend_from_slice(&[134, 0, 0, 0, cur_hop, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    if extracted_mac.is_none() {
        payload.push(1);
        payload.push(1);
        payload.extend_from_slice(&router_mac);
    }
    payload.extend_from_slice(&out_opts);
    Some((router_mac, payload))
}

#[allow(dead_code)]
pub(crate) fn icmpv6_checksum(
    src_ip: std::net::Ipv6Addr,
    dst_ip: std::net::Ipv6Addr,
    icmp_payload: &[u8],
) -> u16 {
    let mut sum: u32 = 0;
    for chunk in src_ip.octets().chunks_exact(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    for chunk in dst_ip.octets().chunks_exact(2) {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    let len = icmp_payload.len() as u32;
    sum += len >> 16;
    sum += len & 0xffff;
    sum += 58u32; // IPPROTO_ICMPV6

    let mut chunks = icmp_payload.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let Some(&rem) = chunks.remainder().first() {
        sum += u16::from_be_bytes([rem, 0]) as u32;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[allow(dead_code)]
pub(crate) fn build_eth_ipv6_ra_frame(
    dst_mac: [u8; 6],
    src_mac: [u8; 6],
    src_ip: std::net::Ipv6Addr,
    dst_ip: std::net::Ipv6Addr,
    icmp_template: &[u8],
) -> Vec<u8> {
    let mut icmp = icmp_template.to_vec();
    icmp[2] = 0;
    icmp[3] = 0;
    let csum = icmpv6_checksum(src_ip, dst_ip, &icmp);
    icmp[2..4].copy_from_slice(&csum.to_be_bytes());

    let payload_len = (icmp.len() as u16).to_be_bytes();
    let mut frame = Vec::with_capacity(14 + 40 + icmp.len());
    // Ethernet header (14B)
    frame.extend_from_slice(&dst_mac);
    frame.extend_from_slice(&src_mac);
    frame.extend_from_slice(&0x86DDu16.to_be_bytes());
    // IPv6 header (40B): Version=6, TC=0, Flow=0, PayloadLen, NextHdr=58, HopLimit=255
    frame.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]);
    frame.extend_from_slice(&payload_len);
    frame.push(58);
    frame.push(255);
    frame.extend_from_slice(&src_ip.octets());
    frame.extend_from_slice(&dst_ip.octets());
    // ICMPv6 payload
    frame.extend_from_slice(&icmp);
    frame
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::{build_eth_ipv6_ra_frame, build_zero_lifetime_ra_payload};
    use dashmap::DashMap;
    use log::{debug, info, warn};
    use std::collections::HashMap;
    use std::io;
    use std::mem;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    use tokio::io::unix::AsyncFd;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static CLIENT_TTL_SECS: AtomicU64 = AtomicU64::new(300);
    static CLIENT_SEEN: OnceLock<DashMap<Ipv4Addr, Instant>> = OnceLock::new();
    static CLIENT_NOTIFY: OnceLock<mpsc::Sender<Ipv4Addr>> = OnceLock::new();

    const ALL_NODES_MULTICAST: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);
    const ALL_ROUTERS_MULTICAST: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 2);

    struct RouterRaTemplate {
        router_mac: [u8; 6],
        icmp_payload: Vec<u8>,
    }

    struct TrackedClient {
        mac: [u8; 6],
        link_local_v6: Option<Ipv6Addr>,
        last_seen: Instant,
    }

    pub fn enable_ra_suppression(ttl: Duration, shutdown: CancellationToken) {
        CLIENT_TTL_SECS.store(ttl.as_secs().max(10), Ordering::Relaxed);
        if ENABLED.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = CLIENT_SEEN.get_or_init(DashMap::new);
        let (tx, rx) = mpsc::channel::<Ipv4Addr>(256);
        let _ = CLIENT_NOTIFY.set(tx);

        tokio::spawn(async move {
            if let Err(e) = run_suppressor(rx, shutdown).await {
                warn!("ra_suppress task exited: {}", e);
            }
        });
    }

    pub fn note_client_ip(ip: IpAddr) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let v4 = match ip {
            IpAddr::V4(v4) => v4,
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => v4,
                None => return,
            },
        };
        if !v4.is_private() || v4.is_loopback() || v4.is_unspecified() {
            return;
        }
        let Some(seen_map) = CLIENT_SEEN.get() else {
            return;
        };
        let now = Instant::now();
        if let Some(mut prev) = seen_map.get_mut(&v4) {
            if now.saturating_duration_since(*prev) < Duration::from_secs(5) {
                return;
            }
            *prev = now;
        } else {
            seen_map.insert(v4, now);
        }
        if let Some(tx) = CLIENT_NOTIFY.get() {
            let _ = tx.try_send(v4);
        }
    }

    fn parse_default_route() -> Option<(String, Ipv4Addr)> {
        let content = std::fs::read_to_string("/proc/net/route").ok()?;
        for line in content.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[1] == "00000000" {
                let ifname = parts[0].to_string();
                if let Ok(gw_hex) = u32::from_str_radix(parts[2], 16) {
                    let gw_ip = Ipv4Addr::from(gw_hex.to_be());
                    return Some((ifname, gw_ip));
                }
            }
        }
        None
    }

    fn parse_mac(s: &str) -> Option<[u8; 6]> {
        let mut mac = [0u8; 6];
        let mut idx = 0;
        for part in s.split(':') {
            if idx >= 6 {
                return None;
            }
            mac[idx] = u8::from_str_radix(part, 16).ok()?;
            idx += 1;
        }
        if idx != 6 || mac == [0u8; 6] || mac == [0xff; 6] {
            return None;
        }
        Some(mac)
    }

    fn format_mac(mac: &[u8; 6]) -> String {
        format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        )
    }

    fn read_arp_table(ifname: &str) -> HashMap<Ipv4Addr, [u8; 6]> {
        let mut map = HashMap::new();
        let Ok(content) = std::fs::read_to_string("/proc/net/arp") else {
            return map;
        };
        for line in content.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 6 && parts[5] == ifname && parts[2] != "0x0" {
                if let (Ok(ip), Some(mac)) = (parts[0].parse::<Ipv4Addr>(), parse_mac(parts[3])) {
                    map.insert(ip, mac);
                }
            }
        }
        map
    }

    fn send_raw_eth_frame(fd: libc::c_int, ifindex: u32, dst_mac: [u8; 6], frame: &[u8]) {
        unsafe {
            let mut sll: libc::sockaddr_ll = mem::zeroed();
            sll.sll_family = libc::AF_PACKET as u16;
            sll.sll_protocol = (libc::ETH_P_IPV6 as u16).to_be();
            sll.sll_ifindex = ifindex as i32;
            sll.sll_halen = 6;
            sll.sll_addr[..6].copy_from_slice(&dst_mac);

            let _ = libc::sendto(
                fd,
                frame.as_ptr() as *const libc::c_void,
                frame.len(),
                0,
                &sll as *const libc::sockaddr_ll as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            );
        }
    }

    fn send_suppression_for_client(
        tx_fd: libc::c_int,
        ifindex: u32,
        client: &TrackedClient,
        routers: &HashMap<Ipv6Addr, RouterRaTemplate>,
    ) {
        for (&router_v6, tpl) in routers {
            if client.mac == tpl.router_mac {
                continue;
            }
            let frame_mc = build_eth_ipv6_ra_frame(
                client.mac,
                tpl.router_mac,
                router_v6,
                ALL_NODES_MULTICAST,
                &tpl.icmp_payload,
            );
            send_raw_eth_frame(tx_fd, ifindex, client.mac, &frame_mc);

            if let Some(client_v6) = client.link_local_v6 {
                let frame_uc = build_eth_ipv6_ra_frame(
                    client.mac,
                    tpl.router_mac,
                    router_v6,
                    client_v6,
                    &tpl.icmp_payload,
                );
                send_raw_eth_frame(tx_fd, ifindex, client.mac, &frame_uc);
            }
        }
    }

    fn send_router_solicitation(icmp6_fd: libc::c_int, ifindex: u32) {
        unsafe {
            let rs: [u8; 8] = [133, 0, 0, 0, 0, 0, 0, 0];
            let mut sin6: libc::sockaddr_in6 = mem::zeroed();
            sin6.sin6_family = libc::AF_INET6 as u16;
            sin6.sin6_addr.s6_addr = ALL_ROUTERS_MULTICAST.octets();
            sin6.sin6_scope_id = ifindex;
            let _ = libc::sendto(
                icmp6_fd,
                rs.as_ptr() as *const libc::c_void,
                rs.len(),
                0,
                &sin6 as *const libc::sockaddr_in6 as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
            );
        }
    }

    fn open_icmp6_socket(ifname: &str) -> io::Result<(AsyncFd<OwnedFd>, u32)> {
        let c_ifname = std::ffi::CString::new(ifname)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid interface name"))?;
        let ifindex = unsafe { libc::if_nametoindex(c_ifname.as_ptr()) };
        if ifindex == 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = unsafe {
            libc::socket(
                libc::AF_INET6,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                libc::IPPROTO_ICMPV6,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        unsafe {
            let hops: libc::c_int = 255;
            let _ = libc::setsockopt(
                fd,
                libc::IPPROTO_IPV6,
                libc::IPV6_MULTICAST_HOPS,
                &hops as *const _ as *const libc::c_void,
                mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
            let _ = libc::setsockopt(
                fd,
                libc::IPPROTO_IPV6,
                libc::IPV6_UNICAST_HOPS,
                &hops as *const _ as *const libc::c_void,
                mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
            let _ = libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_BINDTODEVICE,
                c_ifname.as_ptr() as *const libc::c_void,
                (ifname.len() + 1) as libc::socklen_t,
            );
        }
        Ok((AsyncFd::new(owned)?, ifindex))
    }

    fn open_tx_packet_socket() -> io::Result<OwnedFd> {
        let fd = unsafe {
            libc::socket(
                libc::AF_PACKET,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn recv_icmp6(fd: libc::c_int, buf: &mut [u8]) -> io::Result<(usize, Ipv6Addr)> {
        unsafe {
            let mut sin6: libc::sockaddr_in6 = mem::zeroed();
            let mut addrlen = mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t;
            let n = libc::recvfrom(
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
                &mut sin6 as *mut libc::sockaddr_in6 as *mut libc::sockaddr,
                &mut addrlen,
            );
            if n < 0 {
                return Err(io::Error::last_os_error());
            }
            let src_ip = Ipv6Addr::from(sin6.sin6_addr.s6_addr);
            Ok((n as usize, src_ip))
        }
    }

    fn extract_nd_mac(raw_icmp: &[u8]) -> Option<[u8; 6]> {
        // RS (133) header is 8B; NS (135) / NA (136) header is 24B
        let mut pos = match raw_icmp.first()? {
            133 => 8,
            135 | 136 => 24,
            _ => return None,
        };
        while pos + 2 <= raw_icmp.len() {
            let otype = raw_icmp[pos];
            let olen = (raw_icmp[pos + 1] as usize) * 8;
            if olen == 0 || pos + olen > raw_icmp.len() {
                break;
            }
            if (otype == 1 || otype == 2) && olen >= 8 {
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&raw_icmp[pos + 2..pos + 8]);
                if mac != [0u8; 6] && mac != [0xff; 6] {
                    return Some(mac);
                }
            }
            pos += olen;
        }
        None
    }

    fn sync_and_prune_clients(
        ifname: &str,
        gw_v4: Ipv4Addr,
        clients: &mut HashMap<Ipv4Addr, TrackedClient>,
        mac_to_ll6: &HashMap<[u8; 6], Ipv6Addr>,
    ) {
        let ttl = Duration::from_secs(CLIENT_TTL_SECS.load(Ordering::Relaxed));
        let now = Instant::now();
        let arp = read_arp_table(ifname);
        if let Some(seen_map) = CLIENT_SEEN.get() {
            seen_map.retain(|_, &mut seen_at| now.saturating_duration_since(seen_at) <= ttl);
            for item in seen_map.iter() {
                let ip = *item.key();
                let seen_at = *item.value();
                if ip == gw_v4
                    || crate::listener::tproxy::is_local_host_ip(&IpAddr::V4(ip))
                    || now.saturating_duration_since(seen_at) > ttl
                {
                    continue;
                }
                if let Some(&mac) = arp.get(&ip) {
                    let ll6 = mac_to_ll6.get(&mac).copied();
                    let is_new = !clients.contains_key(&ip);
                    let entry = clients.entry(ip).or_insert(TrackedClient {
                        mac,
                        link_local_v6: ll6,
                        last_seen: seen_at,
                    });
                    entry.mac = mac;
                    if ll6.is_some() {
                        entry.link_local_v6 = ll6;
                    }
                    if seen_at > entry.last_seen {
                        entry.last_seen = seen_at;
                    }
                    if is_new {
                        info!(
                            "ra_suppress: auto-enrolled client {} ({}) for IPv6 RA/RDNSS suppression (ttl={}s)",
                            ip,
                            format_mac(&mac),
                            ttl.as_secs()
                        );
                    }
                }
            }
        }
        clients.retain(|ip, c| {
            let active = now.saturating_duration_since(c.last_seen) <= ttl;
            if !active {
                info!(
                    "ra_suppress: client {} ({}) inactive for >{}s, removed from suppression list",
                    ip,
                    format_mac(&c.mac),
                    ttl.as_secs()
                );
            }
            active
        });
    }

    async fn run_suppressor(
        mut client_rx: mpsc::Receiver<Ipv4Addr>,
        shutdown: CancellationToken,
    ) -> io::Result<()> {
        let (ifname, gw_v4) = parse_default_route().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not detect default LAN route",
            )
        })?;
        let (icmp6_async, ifindex) = open_icmp6_socket(&ifname)?;
        let tx_sock = open_tx_packet_socket()?;

        info!(
            "ra_suppress: enabled on interface {} (ifindex={}, main_router={}, client_ttl={}s)",
            ifname,
            ifindex,
            gw_v4,
            CLIENT_TTL_SECS.load(Ordering::Relaxed)
        );

        let mut routers: HashMap<Ipv6Addr, RouterRaTemplate> = HashMap::new();
        let mut clients: HashMap<Ipv4Addr, TrackedClient> = HashMap::new();
        let mut mac_to_ll6: HashMap<[u8; 6], Ipv6Addr> = HashMap::new();

        // Solicit main router RA immediately at startup
        send_router_solicitation(icmp6_async.get_ref().as_raw_fd(), ifindex);

        let mut ticker = tokio::time::interval(Duration::from_secs(15));
        ticker.tick().await;
        let mut buf = [0u8; 2048];

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    info!("ra_suppress: shutting down");
                    break;
                }
                Some(client_ip) = client_rx.recv() => {
                    if client_ip == gw_v4 || crate::listener::tproxy::is_local_host_ip(&IpAddr::V4(client_ip)) {
                        continue;
                    }
                    let arp = read_arp_table(&ifname);
                    if let Some(&mac) = arp.get(&client_ip) {
                        let ll6 = mac_to_ll6.get(&mac).copied();
                        let is_new = !clients.contains_key(&client_ip);
                        let entry = clients.entry(client_ip).or_insert(TrackedClient {
                            mac,
                            link_local_v6: ll6,
                            last_seen: Instant::now(),
                        });
                        entry.mac = mac;
                        if ll6.is_some() {
                            entry.link_local_v6 = ll6;
                        }
                        entry.last_seen = Instant::now();
                        if is_new {
                            info!(
                                "ra_suppress: auto-enrolled client {} ({}) for IPv6 RA/RDNSS suppression (ttl={}s)",
                                client_ip,
                                format_mac(&mac),
                                CLIENT_TTL_SECS.load(Ordering::Relaxed)
                            );
                        }
                        if routers.is_empty() {
                            send_router_solicitation(icmp6_async.get_ref().as_raw_fd(), ifindex);
                        } else {
                            send_suppression_for_client(tx_sock.as_raw_fd(), ifindex, entry, &routers);
                        }
                    }
                }
                guard_res = icmp6_async.readable() => {
                    let Ok(mut guard) = guard_res else { continue; };
                    let Ok(res) = guard.try_io(|inner| recv_icmp6(inner.get_ref().as_raw_fd(), &mut buf)) else {
                        continue;
                    };
                    let Ok((n, src_v6)) = res else { continue; };
                    if n < 8 || !src_v6.is_unicast_link_local() {
                        continue;
                    }
                    let pkt = &buf[..n];
                    match pkt[0] {
                        134 => {
                            // Router Advertisement
                            let arp = read_arp_table(&ifname);
                            let gw_mac = arp.get(&gw_v4).copied();
                            if let Some((router_mac, zero_payload)) = build_zero_lifetime_ra_payload(pkt, gw_mac) {
                                let is_new_router = !routers.contains_key(&src_v6);
                                routers.insert(
                                    src_v6,
                                    RouterRaTemplate {
                                        router_mac,
                                        icmp_payload: zero_payload,
                                    },
                                );
                                sync_and_prune_clients(&ifname, gw_v4, &mut clients, &mac_to_ll6);
                                if is_new_router {
                                    info!(
                                        "ra_suppress: captured main router RA from {} ({}), suppressing for {} active clients",
                                        src_v6,
                                        format_mac(&router_mac),
                                        clients.len()
                                    );
                                } else {
                                    debug!(
                                        "ra_suppress: intercepted multicast RA from {}, countering for {} active clients",
                                        src_v6,
                                        clients.len()
                                    );
                                }
                                for client in clients.values() {
                                    send_suppression_for_client(tx_sock.as_raw_fd(), ifindex, client, &routers);
                                }
                            }
                        }
                        133 | 135 | 136 => {
                            // RS / NS / NA from a LAN host: learn its link-local IPv6 -> MAC mapping
                            if let Some(mac) = extract_nd_mac(pkt) {
                                mac_to_ll6.insert(mac, src_v6);
                                sync_and_prune_clients(&ifname, gw_v4, &mut clients, &mac_to_ll6);
                                for client in clients.values_mut() {
                                    if client.mac == mac {
                                        client.link_local_v6 = Some(src_v6);
                                        if pkt[0] == 133 {
                                            send_suppression_for_client(tx_sock.as_raw_fd(), ifindex, client, &routers);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ = ticker.tick() => {
                    if routers.is_empty() {
                        send_router_solicitation(icmp6_async.get_ref().as_raw_fd(), ifindex);
                        continue;
                    }
                    sync_and_prune_clients(&ifname, gw_v4, &mut clients, &mac_to_ll6);
                    for client in clients.values() {
                        send_suppression_for_client(tx_sock.as_raw_fd(), ifindex, client, &routers);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub use linux_impl::{enable_ra_suppression, note_client_ip};

#[cfg(not(target_os = "linux"))]
pub fn enable_ra_suppression(_ttl: Duration, _shutdown: CancellationToken) {}

#[cfg(not(target_os = "linux"))]
pub fn note_client_ip(_ip: IpAddr) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn test_zero_lifetime_ra_builder() {
        // Fake RA: Type=134, Code=0, Csum=0, Hop=64, Flags=0xc0, Lifetime=1800 (0x0708), Reach=0, Retrans=0
        let mut raw = vec![
            134, 0, 0, 0, 64, 0xc0, 0x07, 0x08, 0, 0, 0, 0, 0, 0, 0, 0,
            // Option 1 (Source Link-Layer Address, len=1 -> 8B)
            1, 1, 0x9c, 0x9d, 0x7e, 0x7d, 0x38, 0xce,
            // Option 25 (RDNSS, len=3 -> 24B, lifetime=1200 -> 0x000004b0)
            25, 3, 0, 0, 0x00, 0x00, 0x04, 0xb0, 0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0x9e, 0x9d, 0x7e,
            0xff, 0xfe, 0x7d, 0x38, 0xce,
        ];
        let (mac, zero_ra) = build_zero_lifetime_ra_payload(&raw, None).unwrap();
        assert_eq!(mac, [0x9c, 0x9d, 0x7e, 0x7d, 0x38, 0xce]);
        // Router lifetime must be 0
        assert_eq!(&zero_ra[6..8], &[0, 0]);
        // Flags must be 0
        assert_eq!(zero_ra[5], 0);
        // RDNSS lifetime at offset 16 + 8 + 4 = 28..32 must be 0
        assert_eq!(&zero_ra[28..32], &[0, 0, 0, 0]);
        // Passing an already-zeroed RA returns None
        assert!(build_zero_lifetime_ra_payload(&zero_ra, None).is_none());

        let src: Ipv6Addr = "fe80::9e9d:7eff:fe7d:38ce".parse().unwrap();
        let dst: Ipv6Addr = "ff02::1".parse().unwrap();
        let frame = build_eth_ipv6_ra_frame([1, 2, 3, 4, 5, 6], mac, src, dst, &zero_ra);
        assert_eq!(frame.len(), 14 + 40 + zero_ra.len());
        assert_eq!(icmpv6_checksum(src, dst, &frame[54..]), 0);
        raw.clear();
    }
}

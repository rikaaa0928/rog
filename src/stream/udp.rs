use crate::def::{RunUdpReader, RunUdpWriter, UDPMeta, UDPPacket};
use log::debug;
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::{UdpSocket, lookup_host};
use tokio::sync::Mutex;

/// Shared state between reader and writer halves of a UDP stream.
/// Maintains bidirectional mappings between domain names and resolved IPs
/// so that response packets can carry the original domain name back.
struct UdpResolveState {
    /// Forward: "domain:port" → resolved SocketAddr (for write path)
    domain_to_ip: HashMap<String, SocketAddr>,
    /// Reverse: resolved SocketAddr → "domain" (for read path)
    ip_to_domain: HashMap<SocketAddr, String>,
}

// UdpStream 的包装
pub struct UdpRunStream {
    inner: Arc<UdpSocket>,
    src_addr: String,
    resolve_state: Arc<Mutex<UdpResolveState>>,
}
// 为 MyUdpStream 实现构造方法
impl UdpRunStream {
    pub fn new(stream: Arc<UdpSocket>, src_addr: String) -> Self {
        Self {
            inner: stream,
            src_addr,
            resolve_state: Arc::new(Mutex::new(UdpResolveState {
                domain_to_ip: HashMap::new(),
                ip_to_domain: HashMap::new(),
            })),
        }
    }

    /// Create a reader/writer pair that share the same resolve state.
    /// The writer records domain→IP mappings, the reader uses them in reverse
    /// to restore domain names in response packets.
    pub fn new_pair(
        stream: Arc<UdpSocket>,
        src_addr: String,
    ) -> (UdpRunStream, UdpRunStream) {
        let shared_state = Arc::new(Mutex::new(UdpResolveState {
            domain_to_ip: HashMap::new(),
            ip_to_domain: HashMap::new(),
        }));
        (
            UdpRunStream {
                inner: stream.clone(),
                src_addr: src_addr.clone(),
                resolve_state: shared_state.clone(),
            },
            UdpRunStream {
                inner: stream,
                src_addr,
                resolve_state: shared_state,
            },
        )
    }

    async fn resolve_dst_addr(&self, dst_addr: &str, dst_port: u16) -> io::Result<SocketAddr> {
        let cache_key = format!("{}:{}", dst_addr, dst_port);
        {
            let state = self.resolve_state.lock().await;
            if let Some(addr) = state.domain_to_ip.get(&cache_key).copied() {
                return Ok(addr);
            }
        }

        let is_domain = dst_addr.parse::<IpAddr>().is_err();

        let addr = match dst_addr.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, dst_port),
            Err(_) => {
                let prefers_ipv4 = self.inner.local_addr()?.is_ipv4();
                let resolved: Vec<SocketAddr> = lookup_host((dst_addr, dst_port)).await?.collect();
                resolved
                    .iter()
                    .copied()
                    .find(|addr| addr.is_ipv4() == prefers_ipv4)
                    .or_else(|| resolved.first().copied())
                    .ok_or_else(|| io::Error::other("no resolved UDP destination address"))?
            }
        };

        let mut state = self.resolve_state.lock().await;
        state.domain_to_ip.insert(cache_key, addr);
        // Build reverse mapping only when the original address was a domain name,
        // so the read path can restore it in the response packet.
        if is_domain {
            state
                .ip_to_domain
                .insert(addr, dst_addr.to_string());
        }

        Ok(addr)
    }
}

// 为 MyUdpStream 实现 MyStream trait
#[async_trait::async_trait]
impl RunUdpReader for UdpRunStream {
    async fn read(&mut self) -> std::io::Result<UDPPacket> {
        let src_addr = self.src_addr.clone();
        let src: SocketAddr = src_addr.parse().unwrap();
        let inner = self.inner.clone();

        let mut buf = [0u8; 65536];
        let (n, dst) = inner.recv_from(&mut buf).await?;

        // Restore the original domain name if this IP was resolved from one.
        // This is critical for TUN + Fake DNS: the client needs the domain back
        // so it can map it to the fake IP for the return packet.
        let dst_addr_str = {
            let state = self.resolve_state.lock().await;
            if let Some(domain) = state.ip_to_domain.get(&dst) {
                debug!(
                    "udp read: restored domain {} for resolved IP {}",
                    domain, &dst
                );
                domain.clone()
            } else {
                dst.ip().to_string()
            }
        };

        debug!(
            "udp read from {:?} {:?} bytes: {:?}",
            &src,
            &dst,
            &buf[..n].len()
        );
        Ok(UDPPacket {
            meta: UDPMeta {
                dst_addr: dst_addr_str,
                dst_port: dst.port(),
                src_addr: src.ip().to_string(),
                src_port: src.port(),
            },
            data: buf[..n].to_vec(),
        })
    }
}

#[async_trait::async_trait]
impl RunUdpWriter for UdpRunStream {
    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let inner = self.inner.clone();
        let dst_addr = packet.meta.dst_addr.clone();
        let dst_port = packet.meta.dst_port;
        let data = packet.data.clone();
        let resolved_addr = self.resolve_dst_addr(&dst_addr, dst_port).await?;
        debug!(
            "udp send {:?} to {:?} (requested {}:{})",
            &data.as_slice().len(),
            &resolved_addr,
            &dst_addr,
            &dst_port
        );
        inner.send_to(data.as_slice(), resolved_addr).await?;
        Ok(())
    }
}

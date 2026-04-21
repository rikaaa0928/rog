use crate::def::{RunUdpReader, RunUdpWriter, UDPMeta, UDPPacket};
use log::debug;
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::{UdpSocket, lookup_host};
use tokio::sync::Mutex;

// UdpStream 的包装
pub struct UdpRunStream {
    inner: Arc<UdpSocket>,
    src_addr: String,
    resolved_addrs: Arc<Mutex<HashMap<String, SocketAddr>>>,
}
// 为 MyUdpStream 实现构造方法
impl UdpRunStream {
    pub fn new(stream: Arc<UdpSocket>, src_addr: String) -> Self {
        Self {
            inner: stream,
            src_addr,
            resolved_addrs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn resolve_dst_addr(&self, dst_addr: &str, dst_port: u16) -> io::Result<SocketAddr> {
        let cache_key = format!("{}:{}", dst_addr, dst_port);
        if let Some(addr) = self.resolved_addrs.lock().await.get(&cache_key).copied() {
            return Ok(addr);
        }

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

        self.resolved_addrs.lock().await.insert(cache_key, addr);
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
        debug!(
            "udp read from {:?} {:?} bytes: {:?}",
            &src,
            &dst,
            &buf[..n].len()
        );
        Ok(UDPPacket {
            meta: UDPMeta {
                dst_addr: dst.ip().to_string(),
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

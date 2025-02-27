use std::net::SocketAddr;
use std::sync::Arc;
use log::debug;
use tokio::net::{UdpSocket};
use crate::def::{RunUdpStream, UDPMeta, UDPPacket};


// UdpStream 的包装
pub struct UdpRunStream {
    inner: Arc<UdpSocket>,
    src_addr: String,
}
// 为 MyUdpStream 实现构造方法
impl UdpRunStream {
    pub fn new(stream: UdpSocket, src_addr: String) -> Self {
        Self { inner: Arc::new(stream), src_addr }
    }
}

// 为 MyUdpStream 实现 MyStream trait
#[async_trait::async_trait]
impl RunUdpStream for UdpRunStream {
    async fn read(&self) -> std::io::Result<UDPPacket> {
        let src_addr = self.src_addr.clone();
        let src: SocketAddr = src_addr.parse().unwrap();
        let inner = self.inner.clone();

        let mut buf = [0u8; 65536];
        let (n, dst) = inner.recv_from(&mut buf).await?;
        debug!("udp read from {:?} bytes: {:?}", &src, &buf[..n]);
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

    async fn write(&self, packet: UDPPacket) -> std::io::Result<()> {
        let inner = self.inner.clone();
        let dst_addr = packet.meta.dst_addr.clone();
        let dst_port = packet.meta.dst_port;
        let data = packet.data.clone();
        let addr_str = format!("{}:{}", dst_addr, dst_port);
        debug!("udp send {:?} to {:?}", &data.as_slice(), &addr_str);
        inner.send_to(data.as_slice(), addr_str).await?;
        Ok(())
    }
}
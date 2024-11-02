use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
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
impl RunUdpStream for UdpRunStream {
    fn read(&self) -> Pin<Box<dyn Future<Output=std::io::Result<UDPPacket>> + Send>> {
        let src_addr = self.src_addr.clone();
        let src: SocketAddr = src_addr.parse().unwrap();
        let inner = self.inner.clone();
        Box::pin(async move {
            let mut buf = [0u8; 65536];
            let (n, dst) = inner.recv_from(&mut buf).await?;
            println!("udp read from {:?} bytes: {:?}", &src, &buf[..n]);
            Ok(UDPPacket {
                meta: UDPMeta {
                    dst_addr: dst.ip().to_string(),
                    dst_port: dst.port(),
                    src_addr: src.ip().to_string(),
                    src_port: src.port(),
                },
                data: buf[..n].to_vec(),
            })
        })
    }

    fn write(&self, packet: UDPPacket) -> Pin<Box<dyn Future<Output=std::io::Result<()>> + Send>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let dst_addr = packet.meta.dst_addr.clone();
            let dst_port = packet.meta.dst_port;
            let data = packet.data.clone();
            let addr_str = format!("{}:{}", dst_addr, dst_port);
            println!("udp send {:?} to {:?}", &data.as_slice(), &addr_str);
            inner.send_to(data.as_slice(), addr_str).await?;
            Ok(())
        })
    }
}
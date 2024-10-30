use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UdpSocket};
use crate::def::{RunReadHalf, RunStream, RunUdpStream, RunWriteHalf, UDPMeta, UDPPacket};


// UdpStream 的包装
pub struct UdpRunStream {
    inner: UdpSocket,
}
// 为 MyUdpStream 实现构造方法
impl UdpRunStream {
    pub fn new(stream: UdpSocket) -> Self {
        Self { inner: stream }
    }
}

// 为 MyUdpStream 实现 MyStream trait
impl RunUdpStream for UdpRunStream {
    fn read(&mut self) -> Pin<Box<dyn Future<Output=std::io::Result<UDPPacket>> + Send>> {
        Box::pin(async move {
            Ok(UDPPacket {
                meta: UDPMeta {
                    dst_addr: "".to_string(),
                    dst_port: 0,
                    src_addr: "".to_string(),
                    srv_port: 0,
                },
                data: vec![],
            })
        })
    }

    fn write(&mut self, packet: UDPPacket) -> Pin<Box<dyn Future<Output=std::io::Result<()>> + Send>> {
        Box::pin(async move {
            Ok(())
        })
    }
}
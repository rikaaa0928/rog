use crate::def::{RunUdpReader, RunUdpWriter, UDPMeta, UDPPacket};
use crate::stream::grpc_client::pb::{UdpReq, UdpRes};
use log::debug;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;

// UdpStream 的包装
pub struct UdpRunStream {
    inner: Arc<UdpSocket>,
    src_addr: String,
}
// 为 MyUdpStream 实现构造方法
impl UdpRunStream {
    pub fn new(stream: Arc<UdpSocket>, src_addr: String) -> Self {
        Self {
            inner: stream,
            src_addr,
        }
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
        debug!("udp read from {:?} {:?} bytes: {:?}", &src,&dst, &buf[..n]);
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
        let addr_str = format!("{}:{}", dst_addr, dst_port);
        debug!("udp send {:?} to {:?}", &data.as_slice(), &addr_str);
        inner.send_to(data.as_slice(), addr_str).await?;
        Ok(())
    }
}

impl TryInto<UDPPacket> for UdpReq {
    type Error = ();

    fn try_into(self) -> Result<UDPPacket, Self::Error> {
        Ok(UDPPacket {
            meta: UDPMeta {
                dst_addr: self.dst_addr.unwrap(),
                dst_port: self.dst_port.unwrap() as u16,
                src_addr: self.src_addr.unwrap(),
                src_port: self.src_port.unwrap() as u16,
            },
            data: self.payload.unwrap(),
        })
    }
}

impl UdpReq {
    pub(crate) fn from_packet(packet: UDPPacket, auth: String) -> UdpReq {
        UdpReq {
            auth,
            payload: Some(packet.data),
            dst_addr: Some(packet.meta.dst_addr),
            dst_port: Some(packet.meta.dst_port as u32),
            src_addr: Some(packet.meta.src_addr),
            src_port: Some(packet.meta.src_port as u32),
        }
    }
}

impl TryInto<UDPPacket> for UdpRes {
    type Error = ();

    fn try_into(self) -> Result<UDPPacket, Self::Error> {
        Ok(UDPPacket {
            meta: UDPMeta {
                dst_addr: self.dst_addr.unwrap(),
                dst_port: self.dst_port.unwrap() as u16,
                src_addr: self.src_addr.unwrap(),
                src_port: self.src_port.unwrap() as u16,
            },
            data: self.payload,
        })
    }
}

impl TryFrom<UDPPacket> for UdpRes {
    type Error = ();

    fn try_from(value: UDPPacket) -> Result<Self, Self::Error> {
        Ok(Self {
            payload: value.data,
            dst_addr: Some(value.meta.dst_addr),
            dst_port: Some(value.meta.dst_port as u32),
            src_addr: Some(value.meta.src_addr),
            src_port: Some(value.meta.src_port as u32),
        })
    }
}

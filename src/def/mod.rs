use std::io::{Error, ErrorKind, Result};
use std::future::Future;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use crate::util::RunAddr;

// 定义读取半边的 trait
pub trait RunReadHalf {
    // 使用关联类型来定义返回值，因为 async trait 还不稳定
    type ReadFuture<'a>: Future<Output=Result<usize>> + Send + 'a
    where
        Self: 'a;

    // 返回 Future 的读取方法
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::ReadFuture<'a>;

    fn read_exact<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::ReadFuture<'a>;
}

// 定义写入半边的 trait
pub trait RunWriteHalf {
    type WriteFuture<'a>: Future<Output=Result<()>> + Send + 'a
    where
        Self: 'a;

    // 返回 Future 的写入方法
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Self::WriteFuture<'a>;
}

// 定义流的 trait，用于分割读写
pub trait RunStream {
    type ReadHalf: RunReadHalf;
    type WriteHalf: RunWriteHalf;

    fn split(self) -> (Self::ReadHalf, Self::WriteHalf);
}

pub trait RunConnector {
    type Stream: RunStream;
    type StreamFuture: Future<Output=Result<Self::Stream>> + Send;
    fn connect(&self, addr: String) -> Self::StreamFuture;
}

pub trait RunUdpConnector {
    type UdpStream: RunUdpStream;
    type UdpFuture: Future<Output=Result<Option<Self::UdpStream>>> + Send;
    fn udp_tunnel(&self, src_addr: String) -> Self::UdpFuture;
}

pub trait RunAcceptor {
    type Stream: RunStream;
    type Reader: RunReadHalf;
    type Writer: RunWriteHalf;
    type StreamFuture<'a>: Future<Output=Result<(Self::Stream, SocketAddr)>> + Send + 'a
    where
        Self: 'a;
    type HandshakeFuture<'a>: Future<Output=Result<RunAddr>> + Send + 'a
    where
        Self: 'a;

    type PostHandshakeFuture<'a>: Future<Output=Result<()>> + Send + 'a
    where
        Self: 'a;

    fn accept(&self) -> Self::StreamFuture<'_>;

    fn handshake<'a>(&'a self, r: &'a mut Self::Reader, w: &'a mut Self::Writer) -> Self::HandshakeFuture<'_>;

    fn post_handshake<'a>(&'a self, r: &'a mut Self::Reader, w: &'a mut Self::Writer, error: bool) -> Self::PostHandshakeFuture<'_>;
}

pub trait RunListener {
    type Acceptor: RunAcceptor;
    type AcceptorFuture: Future<Output=Result<Self::Acceptor>> + Send;
    fn listen(addr: String) -> Self::AcceptorFuture;
}

#[derive(Debug)]
pub struct UDPMeta {
    pub dst_addr: String,
    pub dst_port: u16,
    pub src_addr: String,
    pub src_port: u16,
}

#[derive(Debug)]
pub struct UDPPacket {
    pub meta: UDPMeta,
    pub data: Vec<u8>,
}

impl UDPPacket {
    pub fn bytes(&self) -> (Vec<Vec<u8>>, String, String) {
        let port = self.meta.src_port.to_be_bytes();
        let head = [0u8, 0, 0, 1, 0, 0, 0, 0, port[0], port[1]];
        let mut payload = head.to_vec();
        payload.extend(self.data.iter());
        (Vec::from([payload]),
         format!("{}:{}", if self.meta.src_addr == "0.0.0.0" {
             "127.0.0.1"
         } else {
             self.meta.src_addr.as_str()
         }, self.meta.src_port),
         format!("{}:{}", self.meta.dst_addr, self.meta.dst_port))
    }
    pub fn parse(buf: &[u8], src_addr: SocketAddr) -> Result<Self> {
        if buf.len() < 5 {
            return Err(Error::new(ErrorKind::InvalidData, "udp parse packet too short"));
        }
        let frag = buf[2].clone();
        if frag != 0 {
            return Ok(UDPPacket {
                meta: UDPMeta {
                    dst_addr: "".to_string(),
                    dst_port: 0,
                    src_addr: "".to_string(),
                    src_port: 0,
                },
                data: vec![],
            });
        }
        let a_typ = buf[3].clone();
        let a_len: isize = if a_typ == 1 { 4 } else if a_typ == 4 { 16 } else if a_typ == 3 { buf[4].into() } else { -1 };
        let start: usize = if a_typ == 3 { 5 } else { 4 };
        if a_len < 0 {
            return Err(Error::new(ErrorKind::InvalidData, "udp parse invalid addr type"));
        }
        if start as isize + a_len + 2 >= buf.len() as isize {
            return Err(Error::new(ErrorKind::InvalidData, "udp parse packet too short2"));
        }
        let a_len = a_len as usize;
        let dst_addr = buf[start..start + a_len].to_vec();
        let dst_port = buf[start + a_len..start + a_len + 2].to_vec();
        let data = buf[start + a_len + 2..].to_vec();
        let port = u16::from_be_bytes(dst_port.try_into().unwrap());
        let addr = match a_typ {
            // IPv4
            1 => {
                if dst_addr.len() != 4 {
                    return Err(Error::new(ErrorKind::Other, "Not a ipv4"));
                }
                let ip = Ipv4Addr::new(
                    dst_addr[0],
                    dst_addr[1],
                    dst_addr[2],
                    dst_addr[3],
                );
                Ok(ip.to_string())
            }
            // Domain name
            3 => {
                match std::str::from_utf8(&dst_addr) {
                    Ok(domain) => Ok(domain.to_string()),
                    Err(_) => Err(Error::new(ErrorKind::Other, "Not a domain"))
                }
            }
            // IPv6
            4 => {
                if dst_addr.len() != 16 {
                    return Err(Error::new(ErrorKind::Other, "Not a ipv6"));
                }
                let ip = Ipv6Addr::new(
                    u16::from_be_bytes([dst_addr[0], dst_addr[1]]),
                    u16::from_be_bytes([dst_addr[2], dst_addr[3]]),
                    u16::from_be_bytes([dst_addr[4], dst_addr[5]]),
                    u16::from_be_bytes([dst_addr[6], dst_addr[7]]),
                    u16::from_be_bytes([dst_addr[8], dst_addr[9]]),
                    u16::from_be_bytes([dst_addr[10], dst_addr[11]]),
                    u16::from_be_bytes([dst_addr[12], dst_addr[13]]),
                    u16::from_be_bytes([dst_addr[14], dst_addr[15]]),
                );
                Ok(ip.to_string())
            }
            _ => Err(std::io::Error::new(std::io::ErrorKind::Other, "a_type not found"))
        }?;

        Ok(UDPPacket {
            meta: UDPMeta {
                dst_addr: addr,
                dst_port: port,
                src_addr: src_addr.ip().to_string(),
                src_port: src_addr.port(),
            },
            data,
        })
    }
}

// 定义流的 trait，用于分割读写
pub trait RunUdpStream {
    // 返回 Future 的读取方法
    fn read(&self) -> Pin<Box<dyn Future<Output=Result<UDPPacket>> + Send>>;

    // 返回 Future 的写入方法
    fn write(&self, packet: UDPPacket) -> Pin<Box<dyn Future<Output=Result<()>> + Send>>;
}
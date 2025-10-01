pub mod config;

use crate::util::RunAddr;
use std::io::{Error, ErrorKind, Result};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

#[async_trait::async_trait]
pub trait RunReadHalf: Send {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<usize>;
    async fn handshake(&self) -> Result<Option<(RunAddr, String)>>;
}

#[async_trait::async_trait]
pub trait RunWriteHalf: Send {
    async fn write(&mut self, buf: &[u8]) -> Result<()>;
}

pub trait RunStream: Send {
    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>);
}

pub enum RunAccStream {
    TCPStream(Box<dyn RunStream>),
    UDPSocket((Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)),
}

#[async_trait::async_trait]
pub trait RunConnector: Send + Sync {
    async fn connect(&self, addr: String) -> Result<Box<dyn RunStream>>;

    async fn udp_tunnel(
        &self,
        src_addr: String,
    ) -> Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>>;
}

#[async_trait::async_trait]
pub trait RunAcceptor: Send + Sync {
    async fn accept(&self) -> Result<(RunAccStream, SocketAddr)>;

    // stream handshake
    async fn handshake(
        &self,
        r: &mut dyn RunReadHalf,
        w: &mut dyn RunWriteHalf,
    ) -> Result<(RunAddr, Option<Vec<u8>>)>;

    // stream post handshake
    async fn post_handshake(
        &self,
        _: &mut dyn RunReadHalf,
        _: &mut dyn RunWriteHalf,
        _: bool,
        _: u16,
    ) -> Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait RunListener: Send {
    async fn listen(&self, addr: &str) -> Result<Box<dyn RunAcceptor>>;
}
#[async_trait::async_trait]
pub trait RouterSet: Send + Sync {
    async fn route(&self, l_name: &str, r_name: &str, addr: &RunAddr) -> String;
}

// #[async_trait::async_trait]
// pub trait RunUdpStream: Send + Sync {
//     async fn read(&self) -> Result<UDPPacket>;
//     async fn write(&self, packet: UDPPacket) -> Result<()>;
// }

#[async_trait::async_trait]
pub trait RunUdpReader: Send {
    async fn read(&mut self) -> Result<UDPPacket>;
}

#[async_trait::async_trait]
pub trait RunUdpWriter: Send {
    async fn write(&self, packet: UDPPacket) -> Result<()>;
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
    pub fn reply_bytes(&self) -> (Vec<Vec<u8>>, String, String) {
        let port = self.meta.src_port.to_be_bytes();
        let head = [0u8, 0, 0, 1, 0, 0, 0, 0, port[0], port[1]];
        let mut payload = head.to_vec();
        payload.extend(self.data.iter());
        (
            Vec::from([payload]),
            format!(
                "{}:{}",
                if self.meta.src_addr == "0.0.0.0" {
                    "127.0.0.1"
                } else {
                    self.meta.src_addr.as_str()
                },
                self.meta.src_port
            ),
            format!("{}:{}", self.meta.dst_addr, self.meta.dst_port),
        )
    }
    pub fn parse(buf: &[u8], src_addr: SocketAddr) -> Result<Self> {
        if buf.len() < 5 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "udp parse packet too short",
            ));
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
        let a_len: isize = if a_typ == 1 {
            4
        } else if a_typ == 4 {
            16
        } else if a_typ == 3 {
            buf[4].into()
        } else {
            -1
        };
        let start: usize = if a_typ == 3 { 5 } else { 4 };
        if a_len < 0 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "udp parse invalid addr type",
            ));
        }
        if start as isize + a_len + 2 >= buf.len() as isize {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "udp parse packet too short2",
            ));
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
                let ip = Ipv4Addr::new(dst_addr[0], dst_addr[1], dst_addr[2], dst_addr[3]);
                Ok(ip.to_string())
            }
            // Domain name
            3 => match std::str::from_utf8(&dst_addr) {
                Ok(domain) => Ok(domain.to_string()),
                Err(_) => Err(Error::new(ErrorKind::Other, "Not a domain")),
            },
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
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "a_type not found",
            )),
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

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use crate::def::RunReadHalf;
use crate::stream::tcp::TcpReadHalf;
use crate::util::socks5::{CMD_CONNECT, CMD_UDP};

pub struct Request {
    pub version: u8,
    pub cmd: u8,
    pub rsv: u8,
    pub a_typ: u8,
    pub dst_addr: Vec<u8>,
    pub dst_port: [u8; 2],
}

impl Request {
    pub fn parse<'a>(stream: &'a mut TcpReadHalf) -> Pin<Box<dyn Future<Output=std::io::Result<Self>> + Send + 'a>> {
        Box::pin(async move {
            let mut buf = [0u8; 1];
            let _ = stream.read_exact(&mut buf).await?;
            let version = buf[0].clone();
            if version != 5 {
                return Err(std::io::Error::new(std::io::ErrorKind::Other, "invalid socks version"));
            }

            let _ = stream.read_exact(&mut buf).await?;
            let cmd = buf[0].clone();
            if cmd != CMD_CONNECT && cmd != CMD_UDP {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid socks cmd, only CONNECT and UDP ASSOCIATE supported"));
            }
            let _ = stream.read_exact(&mut buf).await?;
            let rsv = buf[0].clone();
            let _ = stream.read_exact(&mut buf).await?;
            let a_typ = buf[0].clone();
            let a_len = if a_typ == 1 { 4 } else if a_typ == 4 { 16 } else if a_typ == 3 { 0 } else { -1 };
            if a_len < 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid addr type"));
            }
            let mut dst_addr = if a_len != 0 {
                let mut t_dst_addr = vec![0u8; a_len as usize];
                let _ = stream.read_exact(&mut t_dst_addr).await?;
                t_dst_addr
            } else {
                let _ = stream.read_exact(&mut buf).await?;
                let len = buf[0].clone();
                let mut t_dst_addr = vec![0u8; len as usize];
                let _ = stream.read_exact(&mut t_dst_addr).await?;
                t_dst_addr
            };
            let mut dst_port = [0u8; 2];
            let _ = stream.read_exact(&mut dst_port).await?;
            Ok(Self {
                version,
                cmd,
                rsv,
                a_typ,
                dst_addr,
                dst_port,
            })
        })
    }
}

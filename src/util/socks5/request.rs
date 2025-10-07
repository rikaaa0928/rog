use crate::def::RunStream;
use crate::util::socks5::{CMD_CONNECT, CMD_UDP};
use std::future::Future;
use std::pin::Pin;

#[allow(dead_code)]
pub struct Request {
    pub version: u8,
    pub cmd: u8,
    pub rsv: u8,
    pub a_typ: u8,
    pub dst_addr: Vec<u8>,
    pub dst_port: [u8; 2],
}

impl Request {
    pub fn parse<'a>(
        stream: &'a mut dyn RunStream,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Self>> + Send + 'a>> {
        Box::pin(async move {
            let mut buf = [0u8; 1];
            let _ = stream.read_exact(&mut buf).await?;
            let version = buf[0].clone();
            if version != 5 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "invalid socks version",
                ));
            }

            let _ = stream.read_exact(&mut buf).await?;
            let cmd = buf[0].clone();
            if cmd != CMD_CONNECT && cmd != CMD_UDP {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid socks cmd, only CONNECT and UDP ASSOCIATE supported",
                ));
            }
            let _ = stream.read_exact(&mut buf).await?;
            let rsv = buf[0].clone();
            let _ = stream.read_exact(&mut buf).await?;
            let a_typ = buf[0].clone();
            let a_len = if a_typ == 1 {
                4
            } else if a_typ == 4 {
                16
            } else if a_typ == 3 {
                0
            } else {
                -1
            };
            if a_len < 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid addr type",
                ));
            }
            let dst_addr = if a_len != 0 {
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

    pub fn parse_bytes(data: &Vec<u8>) -> std::io::Result<(Self, usize)> {
        let mut current = 0;
        if data.len() < current + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no enough data",
            ));
        }
        let mut buf = data[current..current + 1].to_vec();
        current += 1;
        let version = buf[0].clone();
        if version != 5 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "invalid socks version",
            ));
        }
        if data.len() < current + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no enough data",
            ));
        }
        buf = data[current..current + 1].to_vec();
        current += 1;
        let cmd = buf[0].clone();
        if cmd != CMD_CONNECT && cmd != CMD_UDP {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid socks cmd, only CONNECT and UDP ASSOCIATE supported",
            ));
        }
        if data.len() < current + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no enough data",
            ));
        }
        buf = data[current..current + 1].to_vec();
        current += 1;
        let rsv = buf[0].clone();
        if data.len() < current + 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no enough data",
            ));
        }
        buf = data[current..current + 1].to_vec();
        current += 1;
        let a_typ = buf[0].clone();
        let a_len: i32 = if a_typ == 1 {
            4
        } else if a_typ == 4 {
            16
        } else if a_typ == 3 {
            0
        } else {
            -1
        };
        if a_len < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid addr type",
            ));
        }
        let dst_addr = if a_len != 0 {
            if data.len() < current + a_len as usize {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "no enough data",
                ));
            }
            let t_dst_addr = data[current..current + a_len as usize].to_vec();
            current += a_len as usize;
            t_dst_addr
        } else {
            if data.len() < current + 1 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "no enough data",
                ));
            }
            buf = data[current..current + 1].to_vec();
            current += 1;
            let len = buf[0].clone();
            if data.len() < current + len as usize {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "no enough data",
                ));
            }
            let t_dst_addr = data[current..current + len as usize].to_vec();
            current += len as usize;
            t_dst_addr
        };
        if data.len() < current + 2 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no enough data",
            ));
        }
        let dst_port = data[current..current + 2].to_vec();
        let dst_port = [dst_port[0], dst_port[1]];
        current += 2;
        Ok((
            Self {
                version,
                cmd,
                rsv,
                a_typ,
                dst_addr,
                dst_port,
            },
            current,
        ))
    }
}

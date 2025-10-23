use crate::def::RunStream;
use crate::util::socks5::parser::{BytesParser, Socks5MessageParser, StreamParser};
use crate::util::socks5::{CMD_CONNECT, CMD_UDP};
use std::future::Future;
use std::io;
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
    async fn parse_inner<P: Socks5MessageParser + Send>(parser: &mut P) -> io::Result<Self> {
        let version = parser.read_u8().await?;
        if version != 5 {
            return Err(io::Error::other(
                "invalid socks version",
            ));
        }

        let cmd = parser.read_u8().await?;
        if cmd != CMD_CONNECT && cmd != CMD_UDP {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid socks cmd, only CONNECT and UDP ASSOCIATE supported",
            ));
        }

        let rsv = parser.read_u8().await?;
        let a_typ = parser.read_u8().await?;
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
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid addr type",
            ));
        }

        let dst_addr = if a_len != 0 {
            parser.read_vec(a_len as usize).await?
        } else {
            let len = parser.read_u8().await?;
            parser.read_vec(len as usize).await?
        };

        let dst_port_u16 = parser.read_u16().await?;
        let dst_port = dst_port_u16.to_be_bytes();

        Ok(Self {
            version,
            cmd,
            rsv,
            a_typ,
            dst_addr,
            dst_port,
        })
    }

    pub fn parse<'a>(
        stream: &'a mut dyn RunStream,
    ) -> Pin<Box<dyn Future<Output = io::Result<Self>> + Send + 'a>> {
        Box::pin(async move {
            let mut parser = StreamParser { stream };
            Self::parse_inner(&mut parser).await
        })
    }

    pub fn parse_bytes(data: &[u8]) -> io::Result<(Self, usize)> {
        let mut parser = BytesParser::new(data);
        let result = futures::executor::block_on(Self::parse_inner(&mut parser));
        result.map(|r| (r, parser.cursor))
    }
}

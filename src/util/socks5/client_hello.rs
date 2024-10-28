use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use crate::def::{RunReadHalf, RunStream};
use crate::stream::tcp::{TcpReadHalf};

struct Socks5ClientHello {
    version: u8,
    method_num: u8,
    methods: Vec<u8>,
}

impl Socks5ClientHello {
    fn parse<'a>(stream: &'a mut TcpReadHalf) -> Pin<Box<dyn Future<Output=std::io::Result<Self>> + 'a>> {
        Box::pin(async move {
            let mut buf = vec![0u8; 1];
            let _ = stream.read_exact(&mut buf).await?;
            let version = buf[0].clone();
            if version != 5 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid socks version"));
            }
            let _ = stream.read_exact(&mut buf).await?;
            let method_num = buf[0].clone();
            let mut methods = vec![0u8; method_num as usize];
            let _ = stream.read_exact(&mut methods).await?;
            Ok(Socks5ClientHello {
                version,
                method_num,
                methods,
            })
        })
    }
}
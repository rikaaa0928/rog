use crate::def::RunStream;
use crate::util::socks5::parser::{BytesParser, Socks5MessageParser, StreamParser};
use std::future::Future;
use std::io;
use std::pin::Pin;

#[allow(dead_code)]
pub struct ClientHello {
    pub version: u8,
    pub method_num: u8,
    pub methods: Vec<u8>,
}

impl ClientHello {
    async fn parse_inner<P: Socks5MessageParser + Send>(parser: &mut P) -> io::Result<Self> {
        let version = parser.read_u8().await?;
        if version != 5 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid socks version",
            ));
        }
        let method_num = parser.read_u8().await?;
        let methods = parser.read_vec(method_num as usize).await?;
        Ok(ClientHello {
            version,
            method_num,
            methods,
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

    pub fn contains(&self, method: u8) -> bool {
        self.methods.contains(&method)
    }
}

use crate::def::{RunReadHalf, RunStream};
use std::future::Future;
use std::pin::Pin;

#[allow(dead_code)]
pub struct ClientHello {
    pub version: u8,
    pub method_num: u8,
    pub methods: Vec<u8>,
}

impl ClientHello {
    pub fn parse<'a>(stream: &'a mut dyn RunStream) -> Pin<Box<dyn Future<Output=std::io::Result<Self>> + Send + 'a>> {
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
            Ok(ClientHello {
                version,
                method_num,
                methods,
            })
        })
    }

    pub fn contains(&self, method: u8) -> bool {
        self.methods.contains(&method)
    }
}
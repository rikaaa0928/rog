use crate::def::RunStream;
use std::future::Future;
use std::pin::Pin;

#[allow(dead_code)]
pub struct ClientHello {
    pub version: u8,
    pub method_num: u8,
    pub methods: Vec<u8>,
}

impl ClientHello {
    pub fn parse<'a>(
        stream: &'a mut dyn RunStream,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Self>> + Send + 'a>> {
        Box::pin(async move {
            let mut buf = vec![0u8; 1];
            let _ = stream.read_exact(&mut buf).await?;
            let version = buf[0].clone();
            if version != 5 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid socks version",
                ));
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
                std::io::ErrorKind::InvalidData,
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
        let method_num = buf[0].clone();
        if data.len() < current + method_num as usize {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no enough data",
            ));
        }
        let methods = data[current..current + method_num as usize].to_vec();
        current += method_num as usize;
        Ok((
            ClientHello {
                version,
                method_num,
                methods,
            },
            current,
        ))
    }

    pub fn contains(&self, method: u8) -> bool {
        self.methods.contains(&method)
    }
}

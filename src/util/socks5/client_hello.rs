use crate::def::RunStream;
use crate::util::socks5::parse_util::*;
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
            // 读取版本号并验证
            let version = read_u8_from_stream(stream).await?;
            validate_socks_version(version)?;

            // 读取方法数量
            let method_num = read_u8_from_stream(stream).await?;

            // 读取方法列表
            let methods = read_bytes_from_stream(stream, method_num as usize).await?;

            Ok(ClientHello {
                version,
                method_num,
                methods,
            })
        })
    }

    pub fn parse_bytes(data: &[u8]) -> std::io::Result<(Self, usize)> {
        let mut current = 0;

        // 读取版本号并验证
        let (version, new_current) = read_u8_from_slice(data, current)?;
        current = new_current;
        validate_socks_version(version)?;

        // 读取方法数量
        let (method_num, new_current) = read_u8_from_slice(data, current)?;
        current = new_current;

        // 读取方法列表
        let (methods, new_current) = read_bytes_from_slice(data, current, method_num as usize)?;
        current = new_current;

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

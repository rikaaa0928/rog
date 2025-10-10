use crate::def::RunStream;
use crate::util::socks5::parse_util::*;
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
    /// 验证命令类型
    fn validate_cmd(cmd: u8) -> std::io::Result<()> {
        if cmd != CMD_CONNECT && cmd != CMD_UDP {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid socks cmd, only CONNECT and UDP ASSOCIATE supported",
            ));
        }
        Ok(())
    }

    /// 根据地址类型获取地址长度
    fn get_addr_length(a_typ: u8) -> std::io::Result<i32> {
        let a_len = match a_typ {
            1 => 4,  // IPv4
            4 => 16, // IPv6
            3 => 0,  // 域名（需要读取长度字节）
            _ => -1, // 无效类型
        };

        if a_len < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid addr type",
            ));
        }

        Ok(a_len)
    }

    /// 从流中读取目标地址
    async fn read_dst_addr_from_stream(
        stream: &mut dyn RunStream,
        a_len: i32,
    ) -> std::io::Result<Vec<u8>> {
        if a_len != 0 {
            // 固定长度地址（IPv4 或 IPv6）
            read_bytes_from_stream(stream, a_len as usize).await
        } else {
            // 域名：先读取长度，再读取内容
            let len = read_u8_from_stream(stream).await?;
            read_bytes_from_stream(stream, len as usize).await
        }
    }

    /// 从字节数组中读取目标地址
    fn read_dst_addr_from_slice(
        data: &[u8],
        current: usize,
        a_len: i32,
    ) -> std::io::Result<(Vec<u8>, usize)> {
        if a_len != 0 {
            // 固定长度地址（IPv4 或 IPv6）
            read_bytes_from_slice(data, current, a_len as usize)
        } else {
            // 域名：先读取长度，再读取内容
            let (len, new_current) = read_u8_from_slice(data, current)?;
            let (addr, final_current) = read_bytes_from_slice(data, new_current, len as usize)?;
            Ok((addr, final_current))
        }
    }

    pub fn parse<'a>(
        stream: &'a mut dyn RunStream,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Self>> + Send + 'a>> {
        Box::pin(async move {
            // 读取并验证版本号
            let version = read_u8_from_stream(stream).await?;
            validate_socks_version(version)?;

            // 读取并验证命令
            let cmd = read_u8_from_stream(stream).await?;
            Self::validate_cmd(cmd)?;

            // 读取保留字节
            let rsv = read_u8_from_stream(stream).await?;

            // 读取并验证地址类型
            let a_typ = read_u8_from_stream(stream).await?;
            let a_len = Self::get_addr_length(a_typ)?;

            // 读取目标地址
            let dst_addr = Self::read_dst_addr_from_stream(stream, a_len).await?;

            // 读取目标端口
            let port_bytes = read_bytes_from_stream(stream, 2).await?;
            let dst_port = [port_bytes[0], port_bytes[1]];

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

    pub fn parse_bytes(data: &[u8]) -> std::io::Result<(Self, usize)> {
        let mut current = 0;

        // 读取并验证版本号
        let (version, new_current) = read_u8_from_slice(data, current)?;
        current = new_current;
        validate_socks_version(version)?;

        // 读取并验证命令
        let (cmd, new_current) = read_u8_from_slice(data, current)?;
        current = new_current;
        Self::validate_cmd(cmd)?;

        // 读取保留字节
        let (rsv, new_current) = read_u8_from_slice(data, current)?;
        current = new_current;

        // 读取并验证地址类型
        let (a_typ, new_current) = read_u8_from_slice(data, current)?;
        current = new_current;
        let a_len = Self::get_addr_length(a_typ)?;

        // 读取目标地址
        let (dst_addr, new_current) = Self::read_dst_addr_from_slice(data, current, a_len)?;
        current = new_current;

        // 读取目标端口
        let (port_bytes, new_current) = read_bytes_from_slice(data, current, 2)?;
        current = new_current;
        let dst_port = [port_bytes[0], port_bytes[1]];

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

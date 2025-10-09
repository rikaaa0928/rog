use crate::def::RunStream;
use std::future::Future;
use std::pin::Pin;

/// 从流中读取指定数量的字节
pub async fn read_bytes_from_stream(
    stream: &mut dyn RunStream,
    size: usize,
) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; size];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

/// 从字节数组中读取指定数量的字节，返回读取的数据和新的偏移量
pub fn read_bytes_from_slice(
    data: &[u8],
    current: usize,
    size: usize,
) -> std::io::Result<(Vec<u8>, usize)> {
    if data.len() < current + size {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no enough data",
        ));
    }
    let bytes = data[current..current + size].to_vec();
    Ok((bytes, current + size))
}

/// 从流中读取单个字节
pub async fn read_u8_from_stream(stream: &mut dyn RunStream) -> std::io::Result<u8> {
    let bytes = read_bytes_from_stream(stream, 1).await?;
    Ok(bytes[0])
}

/// 从字节数组中读取单个字节
pub fn read_u8_from_slice(data: &[u8], current: usize) -> std::io::Result<(u8, usize)> {
    let (bytes, new_current) = read_bytes_from_slice(data, current, 1)?;
    Ok((bytes[0], new_current))
}

/// 验证 SOCKS5 版本号
pub fn validate_socks_version(version: u8) -> std::io::Result<()> {
    if version != 5 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid socks version",
        ));
    }
    Ok(())
}

/// 通用的解析器 trait，用于统一流解析和字节解析的接口
pub trait Socks5Parser: Sized {
    /// 从流中解析
    fn parse<'a>(
        stream: &'a mut dyn RunStream,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Self>> + Send + 'a>>;

    /// 从字节数组中解析，返回解析后的对象和消耗的字节数
    fn parse_bytes(data: &[u8]) -> std::io::Result<(Self, usize)>;
}

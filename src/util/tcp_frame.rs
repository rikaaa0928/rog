use aes_gcm::aead::OsRng;
use aes_gcm::aead::rand_core::RngCore;
use prost::Message;
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const CONN_TYPE_STREAM: u8 = 0x01;
pub const CONN_TYPE_UDP: u8 = 0x02;

pub async fn write_frame<W: AsyncWriteExt + Unpin, M: Message>(
    writer: &mut W,
    msg: &M,
) -> io::Result<()> {
    let data = msg.encode_to_vec();
    let len = data.len() as u64;
    let r = OsRng.next_u64();
    let second = len.wrapping_sub(r);
    writer.write_all(&r.to_be_bytes()).await?;
    writer.write_all(&second.to_be_bytes()).await?;
    writer.write_all(&data).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut buf16 = [0u8; 16];
    reader.read_exact(&mut buf16).await?;
    let r = u64::from_be_bytes(buf16[..8].try_into().unwrap());
    let second = u64::from_be_bytes(buf16[8..].try_into().unwrap());
    let len = r.wrapping_add(second) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(io::Error::other("frame too large"));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

pub async fn read_msg<R: AsyncReadExt + Unpin, M: Message + Default>(
    reader: &mut R,
) -> io::Result<M> {
    let data = read_frame(reader).await?;
    M::decode(data.as_slice()).map_err(|e| io::Error::other(format!("protobuf decode: {}", e)))
}

pub async fn read_conn_type<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<u8> {
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf).await?;
    Ok(buf[0])
}

pub async fn write_conn_type<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    conn_type: u8,
) -> io::Result<()> {
    writer.write_all(&[conn_type]).await?;
    writer.flush().await?;
    Ok(())
}

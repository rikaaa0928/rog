use crate::def::RunStream;
use async_trait::async_trait;
use std::io;

#[async_trait]
pub trait Socks5MessageParser {
    async fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()>;
    async fn read_u8(&mut self) -> io::Result<u8> {
        let mut buf = [0u8; 1];
        self.read_exact(&mut buf).await?;
        Ok(buf[0])
    }
    async fn read_u16(&mut self) -> io::Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf).await?;
        Ok(u16::from_be_bytes(buf))
    }
    async fn read_vec(&mut self, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.read_exact(&mut buf).await?;
        Ok(buf)
    }
}

pub struct StreamParser<'a> {
    pub stream: &'a mut dyn RunStream,
}

#[async_trait]
impl Socks5MessageParser for StreamParser<'_> {
    async fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.stream.read_exact(buf).await?;
        Ok(())
    }
}

pub struct BytesParser<'a> {
    pub data: &'a [u8],
    pub cursor: usize,
}

impl<'a> BytesParser<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, cursor: 0 }
    }
}

#[async_trait]
impl Socks5MessageParser for BytesParser<'_> {
    async fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let len = buf.len();
        if self.data.len() < self.cursor + len {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no enough data"));
        }
        buf.copy_from_slice(&self.data[self.cursor..self.cursor + len]);
        self.cursor += len;
        Ok(())
    }
}

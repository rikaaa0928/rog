use crate::def::{RunReadHalf, RunStream, RunWriteHalf, StreamInfo};
use std::any::Any;
use std::io::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct TcpReadHalf {
    reader: tokio::net::tcp::OwnedReadHalf,
}

// TcpStream 实现的写半边
pub struct TcpWriteHalf {
    writer: tokio::net::tcp::OwnedWriteHalf,
}

// TcpStream 的包装
pub struct TcpRunStream {
    inner: TcpStream,
    info: StreamInfo,
}

// 为 TcpReadHalf 实现 MyReadHalf trait
#[async_trait::async_trait]
impl RunReadHalf for TcpReadHalf {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.reader.read(buf).await
    }
}

// 为 TcpWriteHalf 实现 MyWriteHalf trait
#[async_trait::async_trait]
impl RunWriteHalf for TcpWriteHalf {
    async fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.writer.write_all(buf).await
    }
}

// 为 MyTcpStream 实现构造方法
impl TcpRunStream {
    pub fn new(stream: TcpStream) -> Self {
        Self {
            inner: stream,
            info: StreamInfo::default(),
        }
    }
}

// 为 MyTcpStream 实现 MyStream trait
#[async_trait::async_trait]
impl RunStream for TcpRunStream {
    fn get_info(&self) -> &StreamInfo {
        &self.info
    }

    fn set_info(&mut self, f: &mut dyn FnMut(&mut StreamInfo)) {
        f(&mut self.info)
    }

    fn split(self: Box<Self>) -> (Box<dyn RunReadHalf>, Box<dyn RunWriteHalf>) {
        let (reader, writer) = self.inner.into_split();
        (
            Box::new(TcpReadHalf { reader }),
            Box::new(TcpWriteHalf { writer }),
        )
    }

    fn into_tcp_stream(self: Box<Self>) -> std::result::Result<tokio::net::TcpStream, Box<dyn RunStream>> {
        Ok(self.inner)
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read(buf).await
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    async fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.inner.write_all(buf).await
    }
}

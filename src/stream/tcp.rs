use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use crate::def::{RunReadHalf, RunStream, RunWriteHalf};

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
}

// 为 TcpReadHalf 实现 MyReadHalf trait
impl RunReadHalf for TcpReadHalf {
    type ReadFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<usize>> + Send + 'a>>;

    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::ReadFuture<'a> {
        Box::pin(async move {
            self.reader.read(buf).await
        })
    }
}

// 为 TcpWriteHalf 实现 MyWriteHalf trait
impl RunWriteHalf for TcpWriteHalf {
    type WriteFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<()>> + Send + 'a>>;

    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Self::WriteFuture<'a> {
        Box::pin(async move {
            self.writer.write_all(buf).await
        })
    }
}

// 为 MyTcpStream 实现构造方法
impl TcpRunStream {
    pub fn new(stream: TcpStream) -> Self {
        Self { inner: stream }
    }
}

// 为 MyTcpStream 实现 MyStream trait
impl RunStream for TcpRunStream {
    type ReadHalf = TcpReadHalf;
    type WriteHalf = TcpWriteHalf;

    fn split(self) -> (Self::ReadHalf, Self::WriteHalf) {
        let (reader, writer) = self.inner.into_split();
        (
            TcpReadHalf { reader },
            TcpWriteHalf { writer },
        )
    }
}
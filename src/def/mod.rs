use std::io::{Result};
use std::future::Future;

// 定义读取半边的 trait
pub trait RunReadHalf: Send + Sync {
    // 使用关联类型来定义返回值，因为 async trait 还不稳定
    type ReadFuture<'a>: Future<Output=Result<usize>> + Send + Sync + 'a
    where
        Self: 'a;

    // 返回 Future 的读取方法
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::ReadFuture<'a>;
}

// 定义写入半边的 trait
pub trait RunWriteHalf: Send + Sync {
    type WriteFuture<'a>: Future<Output=Result<()>> + Send + Sync + 'a
    where
        Self: 'a;

    // 返回 Future 的写入方法
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Self::WriteFuture<'a>;
}

// 定义流的 trait，用于分割读写
pub trait RunStream: Send + Sync {
    type ReadHalf: RunReadHalf;
    type WriteHalf: RunWriteHalf;

    fn split(self) -> (Self::ReadHalf, Self::WriteHalf);
}

pub trait RunConnector: Send + Sync {
    type Stream: RunStream;
    type StreamFuture: Future<Output=Result<Self::Stream>> + Send + Sync;

    fn connect(&self, addr: String) -> Self::StreamFuture;
}

pub trait RunAcceptor: Send + Sync {
    type Stream: RunStream;
    type StreamFuture: Future<Output=Result<Self::Stream>> + Send + Sync;
    fn accept(&self) -> Result<Self::StreamFuture>;
}

pub trait RunListener: Send + Sync {
    type Acceptor: RunAcceptor;
    fn listen(addr: String) -> Result<Self::Acceptor>;
}
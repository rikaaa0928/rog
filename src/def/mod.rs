use std::io::{Result};
use std::future::Future;

// 定义读取半边的 trait
pub trait RunReadHalf {
    // 使用关联类型来定义返回值，因为 async trait 还不稳定
    type ReadFuture<'a>: Future<Output=Result<usize>> + Send + 'a
    where
        Self: 'a;

    // 返回 Future 的读取方法
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::ReadFuture<'a>;
}

// 定义写入半边的 trait
pub trait RunWriteHalf {
    type WriteFuture<'a>: Future<Output=Result<()>> + Send + 'a
    where
        Self: 'a;

    // 返回 Future 的写入方法
    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Self::WriteFuture<'a>;
}

// 定义流的 trait，用于分割读写
pub trait RunStream {
    type ReadHalf: RunReadHalf;
    type WriteHalf: RunWriteHalf;

    fn split(self) -> (Self::ReadHalf, Self::WriteHalf);
}

pub trait RunConnector {
    type Stream: RunStream;
    type StreamFuture: Future<Output=Result<Self::Stream>>;

    fn connect(&self, addr: String) -> Self::StreamFuture;
}

pub trait RunAcceptor {
    type Stream: RunStream;
    type StreamFuture: Future<Output=Result<Self::Stream>> + Send;
    fn accept(&self) -> Result<Self::StreamFuture>;
}

pub trait RunListener {
    type Acceptor: RunAcceptor;
    fn listen(addr: String) -> Result<Self::Acceptor>;
}
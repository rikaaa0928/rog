use std::io::{Result};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::util::RunAddr;

// 定义读取半边的 trait
pub trait RunReadHalf {
    // 使用关联类型来定义返回值，因为 async trait 还不稳定
    type ReadFuture<'a>: Future<Output=Result<usize>> + Send + 'a
    where
        Self: 'a;

    // 返回 Future 的读取方法
    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::ReadFuture<'a>;

    fn read_exact<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::ReadFuture<'a>;
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
    type StreamFuture: Future<Output=Result<Self::Stream>> + Send;
    fn connect(&self, addr: String) -> Self::StreamFuture;
}

pub trait RunUdpConnector {
    type UdpStream: RunUdpStream;
    type UdpFuture: Future<Output=Result<Option<Self::UdpStream>>> + Send;
    fn udp_tunnel(&self, addr: SocketAddr) -> Self::UdpFuture;
}

pub trait RunAcceptor {
    type Stream: RunStream;
    type Reader: RunReadHalf;
    type Writer: RunWriteHalf;
    type StreamFuture<'a>: Future<Output=Result<(Self::Stream, SocketAddr)>> + Send + 'a
    where
        Self: 'a;
    type HandshakeFuture<'a>: Future<Output=Result<RunAddr>> + Send + 'a
    where
        Self: 'a;

    type PostHandshakeFuture<'a>: Future<Output=Result<()>> + Send + 'a
    where
        Self: 'a;

    fn accept(&self) -> Self::StreamFuture<'_>;

    fn handshake<'a, T: RunUdpConnector + Send + Sync + 'a>(&'a self, r: &'a mut Self::Reader, w: &'a mut Self::Writer, udp_connector: Option<T>) -> Self::HandshakeFuture<'_>;

    fn post_handshake<'a>(&'a self, r: &'a mut Self::Reader, w: &'a mut Self::Writer, error: bool) -> Self::PostHandshakeFuture<'_>;
}

pub trait RunListener {
    type Acceptor: RunAcceptor;
    type AcceptorFuture: Future<Output=Result<Self::Acceptor>> + Send;
    fn listen(addr: String) -> Self::AcceptorFuture;
}

#[derive(Debug)]
pub struct UDPPacket {
    pub dst_addr: String,
    pub dst_port: u16,
    pub src_addr: String,
    pub srv_port: u16,
    pub data: Vec<u8>,
}

// 定义流的 trait，用于分割读写
pub trait RunUdpStream {
    // 返回 Future 的读取方法
    fn read(&mut self) -> Pin<Box<dyn Future<Output=std::io::Result<UDPPacket>> + Send>>;

    // 返回 Future 的写入方法
    fn write(&mut self, packet: UDPPacket) -> Pin<Box<dyn Future<Output=Result<()>> + Send>>;
}
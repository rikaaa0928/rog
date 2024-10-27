use std::future::Future;
use std::pin::Pin;
use crate::def::{RunAcceptor, RunListener};

pub struct TcpRunAcceptor {}

pub struct TcpRunListener {}

impl RunAcceptor for TcpRunAcceptor {
    type Stream = crate::stream::tcp::TcpRunStream;
    type StreamFuture = Pin<Box<dyn Future<Output=std::io::Result<Self::Stream>>>>;

    fn accept(&self) -> std::io::Result<Self::StreamFuture> {
        todo!()
    }
}

impl RunListener for TcpRunListener {
    type Acceptor = TcpRunAcceptor;

    fn listen(addr: String) -> std::io::Result<Self::Acceptor> {
        todo!()
    }
}
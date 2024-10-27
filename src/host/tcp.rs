use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use tokio::net::TcpListener;
use crate::def::{RunAcceptor, RunListener};
use crate::stream::tcp::{TcpReadHalf, TcpRunStream, TcpWriteHalf};

pub struct TcpRunAcceptor {
    inner: TcpListener,
}

pub struct TcpRunListener {}

impl RunAcceptor for TcpRunAcceptor {
    type Stream = TcpRunStream;
    type Reader = TcpReadHalf;
    type Writer = TcpWriteHalf;
    type StreamFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<(Self::Stream, SocketAddr)>> + Send + 'a>>;
    type HandshakeFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<()>> + Send + 'a>>;

    fn accept(&self) -> Self::StreamFuture<'_> {
        Box::pin(async move {
            let (socket, addr) = self.inner.accept().await?;
            Ok((TcpRunStream::new(socket), addr))
        })
    }

    fn handshake(&self, r: &Self::Reader, w: &Self::Writer) -> Self::HandshakeFuture<'_> {
        Box::pin(async move {
            Ok(())
        })
    }
}

impl RunListener for TcpRunListener {
    type Acceptor = TcpRunAcceptor;
    type AcceptorFuture = Pin<Box<dyn Future<Output=std::io::Result<Self::Acceptor>> + Send>>;

    fn listen(addr: String) -> Self::AcceptorFuture {
        Box::pin(async move {
            let listener = TcpListener::bind(addr).await?;
            Ok(TcpRunAcceptor {
                inner: listener
            })
        })
    }
}
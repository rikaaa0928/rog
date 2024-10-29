use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use crate::def::{RunAcceptor, RunListener, RunUdpConnector};
use crate::stream::tcp::{TcpReadHalf, TcpRunStream, TcpWriteHalf};
use crate::util::RunAddr;

pub struct TcpRunAcceptor {
    inner: TcpListener,
}

pub struct TcpRunListener {}

impl RunAcceptor for TcpRunAcceptor {
    type Stream = TcpRunStream;
    type Reader = TcpReadHalf;
    type Writer = TcpWriteHalf;
    type StreamFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<(Self::Stream, SocketAddr)>> + Send + 'a>>;
    type HandshakeFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<RunAddr>> + Send + 'a>>;
    type PostHandshakeFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<()>> + Send + 'a>>;

    fn accept(&self) -> Self::StreamFuture<'_> {
        Box::pin(async move {
            let (socket, addr) = self.inner.accept().await?;
            Ok((TcpRunStream::new(socket), addr))
        })
    }

    fn handshake<'a, T: RunUdpConnector + Send + Sync + 'a>(&'a self, r: &'a mut Self::Reader, w: &'a mut Self::Writer, udp_connector: Option<T>) -> Self::HandshakeFuture<'_> {
        Box::pin(async move {
            Ok(RunAddr {
                addr: "".to_string(),
                port: 0,
                a_type: 0,
            })
        })
    }

    fn post_handshake<'a>(&'a self, r: &'a mut Self::Reader, w: &'a mut Self::Writer, error: bool) -> Self::PostHandshakeFuture<'_> {
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
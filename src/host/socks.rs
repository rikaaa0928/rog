use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use crate::def::{RunAcceptor, RunListener};
use crate::host::tcp::{TcpRunAcceptor, TcpRunListener};
use crate::stream::tcp::{TcpReadHalf, TcpRunStream, TcpWriteHalf};

struct SocksRunAcceptor {
    inner: TcpRunAcceptor,
    user: Option<String>,
    pw: Option<String>,
}

impl SocksRunAcceptor {
    fn new(a: TcpRunAcceptor, user: Option<String>, pw: Option<String>) -> SocksRunAcceptor {
        SocksRunAcceptor {
            inner: a,
            user,
            pw,
        }
    }
}
impl RunAcceptor for SocksRunAcceptor {
    type Stream = TcpRunStream;
    type Reader = TcpReadHalf;
    type Writer = TcpWriteHalf;
    type StreamFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<(Self::Stream, SocketAddr)>> + Send + 'a>>;
    type HandshakeFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<()>> + Send + 'a>>;

    fn accept(&self) -> Self::StreamFuture<'_> {
        Box::pin(self.inner.accept())
    }

    fn handshake(&self, r: &Self::Reader, w: &Self::Writer) -> Self::HandshakeFuture<'_> {
        Box::pin(async move {


            Ok(())
        })
    }
}
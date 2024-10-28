use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use crate::def::{RunAcceptor, RunListener};
use crate::host::tcp::{TcpRunAcceptor, TcpRunListener};
use crate::stream::tcp::{TcpReadHalf, TcpRunStream, TcpWriteHalf};
use crate::util::RunAddr;
use crate::def::RunWriteHalf;

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
    type HandshakeFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<RunAddr>> + Send + 'a>>;
    type PostHandshakeFuture<'a> = Pin<Box<dyn Future<Output=std::io::Result<()>> + Send + 'a>>;

    fn accept(&self) -> Self::StreamFuture<'_> {
        Box::pin(self.inner.accept())
    }

    fn handshake(&self, r: &mut Self::Reader, w: &mut Self::Writer) -> Self::HandshakeFuture<'_> {
        Box::pin(async move {
            Ok(RunAddr{
                addr: "".to_string(),
                port: 0,
                a_type: 0,
            })
        })
    }

    fn post_handshake<'a>(&'a self, _: &'a mut Self::Reader, w: &'a mut TcpWriteHalf, error: bool) -> Self::PostHandshakeFuture<'_> {
        Box::pin(async move {
            if error {
                w.write(&[0x05, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff]).await?;
            } else {
                w.write(&[0x05, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff]).await?;
            }
            Ok(())
        })
    }
}
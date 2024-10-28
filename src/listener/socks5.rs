use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use crate::def::{RunAcceptor, RunListener};
use crate::listener::tcp::{TcpRunAcceptor, TcpRunListener};
use crate::stream::tcp::{TcpReadHalf, TcpRunStream, TcpWriteHalf};
use crate::util::RunAddr;
use crate::def::RunWriteHalf;
use crate::util;

pub struct SocksRunAcceptor {
    inner: TcpRunAcceptor,
    user: Option<String>,
    pw: Option<String>,
}

impl SocksRunAcceptor {
    pub fn new(a: TcpRunAcceptor, user: Option<String>, pw: Option<String>) -> SocksRunAcceptor {
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

    fn handshake<'a>(&'a self, r: &'a mut Self::Reader, w: &'a mut Self::Writer) -> Self::HandshakeFuture<'_> {
        Box::pin(async move {
            let hello = &util::socks5::client_hello::ClientHello::parse(r).await?;
            if !hello.contains(util::socks5::NO_AUTH) {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "no available authentication found"));
            }
            let hello_back = util::socks5::server_hello::ServerHello::new(hello.version.clone(), util::socks5::NO_AUTH);
            w.write(&hello_back.to_bytes()).await?;
            let req = &util::socks5::request::Request::parse(r).await?;
            req.try_into()
        })
    }

    fn post_handshake<'a>(&'a self, _: &'a mut Self::Reader, w: &'a mut TcpWriteHalf, error: bool) -> Self::PostHandshakeFuture<'_> {
        Box::pin(async move {
            let confirm = util::socks5::confirm::Confirm::new(error);
            w.write(&confirm.to_bytes()).await?;
            Ok(())
        })
    }
}
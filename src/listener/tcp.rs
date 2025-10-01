use crate::def::{RunAccStream, RunAcceptor, RunListener, RunReadHalf, RunWriteHalf};
use crate::stream::tcp::TcpRunStream;
use crate::util::RunAddr;
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub struct TcpRunAcceptor {
    inner: TcpListener,
}

pub struct TcpRunListener {}

#[async_trait::async_trait]
impl RunAcceptor for TcpRunAcceptor {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let (socket, addr) = self.inner.accept().await?;
        Ok((
            RunAccStream::TCPStream(Box::new(TcpRunStream::new(socket))),
            addr,
        ))
    }

    async fn handshake(
        &self,
        _: &mut dyn RunReadHalf,
        _: &mut dyn RunWriteHalf,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        Ok((
            RunAddr {
                addr: "".to_string(),
                port: 0,
                // a_type: 0,
                udp: false,
                // cache: None,
            },
            None,
        ))
    }
}

#[async_trait::async_trait]
impl RunListener for TcpRunListener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Box::new(TcpRunAcceptor { inner: listener }))
    }
}

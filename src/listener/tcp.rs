use crate::def::{RunAccStream, RunAcceptor, RunListener, RunReadHalf, RunStream, RunWriteHalf};
use crate::stream::tcp::TcpRunStream;
use crate::util::RunAddr;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

#[derive(Clone)]
pub struct TcpRunAcceptor {
    inner: Arc<TcpListener>,
}

pub struct TcpRunListener {}

#[async_trait::async_trait]
impl RunAcceptor for TcpRunAcceptor {
    fn box_clone(&self) -> Box<dyn RunAcceptor> {
        Box::new(self.clone())
    }

    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let (socket, addr) = self.inner.accept().await?;
        Ok((
            RunAccStream::TCPStream(Box::new(TcpRunStream::new(socket))),
            addr,
        ))
    }

    async fn handshake(
        &self,
        _stream: &mut dyn RunStream,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        Ok((
            RunAddr {
                addr: "".to_string(),
                port: 0,
                udp: false,
            },
            None,
        ))
    }
}

#[async_trait::async_trait]
impl RunListener for TcpRunListener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Box::new(TcpRunAcceptor {
            inner: Arc::new(listener),
        }))
    }
}
use crate::def::{RunAccStream, RunAcceptor, RunListener, RunStream};
use crate::stream::tcp::TcpRunStream;
use crate::util::RunAddr;
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub struct TcpRunAcceptor {
    inner: TcpListener,
}

pub struct TcpRunListener {}

use std::any::Any;

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
        _stream: &mut dyn RunStream,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>, Box<dyn Any + Send>)> {
        Ok((
            RunAddr {
                addr: "".to_string(),
                port: 0,
                udp: false,
            },
            None,
            Box::new(()),
        ))
    }

    async fn post_handshake(
        &self,
        _stream: &mut dyn RunStream,
        _success: bool,
        _payload_len: usize,
        _state: Box<dyn Any + Send>,
    ) -> std::io::Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl RunListener for TcpRunListener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Box::new(TcpRunAcceptor { inner: listener }))
    }
}

use crate::def::{RunAcceptor, RunStream, RunAccStream};
use crate::listener::http::HttpRunAcceptor;
use crate::listener::socks5::SocksRunAcceptor;
use crate::stream::buffered::BufferedStream;
use crate::util::RunAddr;
use std::io::ErrorKind;
use std::net::SocketAddr;

pub struct AdaptiveRunAcceptor {
    inner: Box<dyn RunAcceptor>,
}

impl Clone for AdaptiveRunAcceptor {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.box_clone(),
        }
    }
}

impl AdaptiveRunAcceptor {
    pub fn new(inner: Box<dyn RunAcceptor>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl RunAcceptor for AdaptiveRunAcceptor {
    fn box_clone(&self) -> Box<dyn RunAcceptor> {
        Box::new(self.clone())
    }

    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        self.inner.accept().await
    }

    async fn handshake(
        &self,
        stream: &mut dyn RunStream,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        let mut buf = vec![0u8; 2048];
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                "Connection closed before handshake",
            ));
        }
        buf.truncate(n);

        let is_socks = buf[0] == 0x05;

        let mut buffered_stream = BufferedStream::new(buf, stream);

        if is_socks {
            let acceptor = SocksRunAcceptor::new(self.inner.box_clone(), None, None);
            acceptor.handshake(&mut buffered_stream).await
        } else {
            let acceptor = HttpRunAcceptor::new(self.inner.box_clone(), None, None);
            acceptor.handshake(&mut buffered_stream).await
        }
    }
}
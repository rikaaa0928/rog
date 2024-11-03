use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
// use std::sync::Arc;
use tokio::net::TcpListener;
// use tokio::sync::Mutex;
use crate::def::{RunAcceptor, RunListener, RunReadHalf, RunStream, RunWriteHalf};
use crate::stream::tcp::{TcpReadHalf, TcpRunStream, TcpWriteHalf};
use crate::util::RunAddr;

pub struct TcpRunAcceptor {
    inner: TcpListener,
}

pub struct TcpRunListener {}

#[async_trait::async_trait]
impl RunAcceptor for TcpRunAcceptor {
    async fn accept(&self) -> std::io::Result<(Box<dyn RunStream>, SocketAddr)> {
        let (socket, addr) = self.inner.accept().await?;
        Ok((Box::new(TcpRunStream::new(socket)), addr))
    }

    async fn handshake(&self, r: &mut dyn RunReadHalf, w: &mut dyn RunWriteHalf) -> std::io::Result<RunAddr> {
        Ok(RunAddr {
            addr: "".to_string(),
            port: 0,
            a_type: 0,
            udp: false,
        })
    }

    async fn post_handshake(
        &self,
        r: &mut dyn RunReadHalf,
        w: &mut dyn RunWriteHalf,
        error: bool,
        port: u16,
    ) -> std::io::Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl RunListener for TcpRunListener {
    async fn listen(addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Box::new(TcpRunAcceptor {
            inner: listener
        }))
    }
}
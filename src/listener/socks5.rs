use crate::def::RunWriteHalf;
use crate::def::{RunAccStream, RunAcceptor, RunReadHalf};
use crate::util;
use crate::util::RunAddr;
use std::net::SocketAddr;

#[allow(dead_code)]
pub struct SocksRunAcceptor {
    inner: Box<dyn RunAcceptor>,
    user: Option<String>,
    pw: Option<String>,
}

impl SocksRunAcceptor {
    pub fn new(
        a: Box<dyn RunAcceptor>,
        user: Option<String>,
        pw: Option<String>,
    ) -> SocksRunAcceptor {
        SocksRunAcceptor { inner: a, user, pw }
    }
}
#[async_trait::async_trait]
impl RunAcceptor for SocksRunAcceptor {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let res = self.inner.accept().await;
        res
    }

    async fn handshake(
        &self,
        r: &mut dyn RunReadHalf,
        w: &mut dyn RunWriteHalf,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        let hello = &util::socks5::client_hello::ClientHello::parse(r).await?;
        if !hello.contains(util::socks5::NO_AUTH) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no available authentication found",
            ));
        }
        let hello_back = util::socks5::server_hello::ServerHello::new(
            hello.version.clone(),
            util::socks5::NO_AUTH,
        );
        w.write(&hello_back.to_bytes()).await?;
        let req = &util::socks5::request::Request::parse(r).await?;
        let ret: std::io::Result<RunAddr> = req.try_into();
        match ret {
            Ok(addr) => Ok((addr, None)),
            Err(e) => Err(e),
        }
    }

    async fn post_handshake(
        &self,
        r: &mut dyn RunReadHalf,
        w: &mut dyn RunWriteHalf,
        error: bool,
        port: u16,
    ) -> std::io::Result<()> {
        let confirm = util::socks5::confirm::Confirm::new(error, port);
        w.write(&confirm.to_bytes()).await?;
        Ok(())
    }
}

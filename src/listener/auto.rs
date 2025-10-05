use crate::def::{RunAcceptor, RunAccStream, RunStream};
use crate::listener::http::HttpRunAcceptor;
use crate::listener::socks5::SocksRunAcceptor;
use crate::util::RunAddr;
use async_trait::async_trait;
use log::info;
use std::any::Any;
use std::io::{self, Error, ErrorKind};
use std::net::SocketAddr;

// The AutoRunAcceptor delegates the handshake to either a SOCKS5 or HTTP acceptor
// based on the first byte of the incoming stream.
pub struct AutoRunAcceptor {
    inner: Box<dyn RunAcceptor>,
}

impl AutoRunAcceptor {
    pub fn new(inner: Box<dyn RunAcceptor>) -> Self {
        Self { inner }
    }
}

// A dummy acceptor used for delegation. The SOCKS5 and HTTP acceptors require an
// inner acceptor, but for delegation purposes, we don't need a real one.
// Its methods are designed to be unreachable.
struct DummyAcceptor;

#[async_trait]
impl RunAcceptor for DummyAcceptor {
    async fn accept(&self) -> io::Result<(RunAccStream, SocketAddr)> {
        unreachable!("DummyAcceptor should not accept connections")
    }

    async fn handshake(
        &self,
        _stream: &mut dyn RunStream,
    ) -> io::Result<(RunAddr, Option<Vec<u8>>, Box<dyn Any + Send>)> {
        unreachable!("DummyAcceptor should not perform handshakes")
    }

    async fn post_handshake(
        &self,
        _stream: &mut dyn RunStream,
        _success: bool,
        _payload_len: usize,
        _state: Box<dyn Any + Send>,
    ) -> io::Result<()> {
        unreachable!("DummyAcceptor should not perform post-handshakes")
    }
}

// State to remember which protocol was detected during the handshake.
pub enum DetectedProtocol {
    Socks5,
    Http,
}

#[async_trait]
impl RunAcceptor for AutoRunAcceptor {
    async fn accept(&self) -> io::Result<(RunAccStream, SocketAddr)> {
        self.inner.accept().await
    }

    async fn handshake(
        &self,
        stream: &mut dyn RunStream,
    ) -> io::Result<(RunAddr, Option<Vec<u8>>, Box<dyn Any + Send>)> {
        let mut buf = [0; 1];
        // Peek at the first byte to decide the protocol, without consuming the byte from the stream.
        stream.peek(&mut buf).await.map_err(|e| {
            Error::new(
                e.kind(),
                format!("Failed to peek stream for protocol sniffing: {}", e),
            )
        })?;

        // SOCKS5 protocol version is 0x05.
        if buf[0] == 0x05 {
            info!("Protocol sniff: SOCKS5 detected");
            // Delegate to the SOCKS5 acceptor's handshake logic.
            let socks_acceptor = SocksRunAcceptor::new(Box::new(DummyAcceptor), None, None);
            let (addr, cache, _state) = socks_acceptor.handshake(stream).await?;
            Ok((addr, cache, Box::new(DetectedProtocol::Socks5)))
        } else {
            info!("Protocol sniff: HTTP detected");
            // Assume HTTP for any other first byte.
            let http_acceptor = HttpRunAcceptor::new(Box::new(DummyAcceptor), None, None);
            let (addr, cache, _state) = http_acceptor.handshake(stream).await?;
            Ok((addr, cache, Box::new(DetectedProtocol::Http)))
        }
    }

    async fn post_handshake(
        &self,
        stream: &mut dyn RunStream,
        success: bool,
        payload_len: usize,
        state: Box<dyn Any + Send>,
    ) -> io::Result<()> {
        // Downcast the state to our specific enum to know which protocol to use.
        let detected_protocol = state
            .downcast::<DetectedProtocol>()
            .map_err(|_| {
                Error::new(
                    ErrorKind::InvalidInput,
                    "Invalid state type for auto acceptor",
                )
            })?;

        // Delegate to the appropriate post-handshake logic.
        match *detected_protocol {
            DetectedProtocol::Socks5 => {
                let socks_acceptor = SocksRunAcceptor::new(Box::new(DummyAcceptor), None, None);
                socks_acceptor
                    .post_handshake(stream, success, payload_len, Box::new(()))
                    .await
            }
            DetectedProtocol::Http => {
                let http_acceptor = HttpRunAcceptor::new(Box::new(DummyAcceptor), None, None);
                http_acceptor
                    .post_handshake(stream, success, payload_len, Box::new(()))
                    .await
            }
        }
    }
}
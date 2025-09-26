use crate::def::{RunAcceptor, RunAccStream, RunReadHalf, RunWriteHalf};
use crate::util;
use crate::util::RunAddr;
use log::debug;
use std::io::{Cursor, ErrorKind, Read};
use std::net::SocketAddr;
use tokio::sync::Mutex;
use url::Url;

struct PreReadStream<'a> {
    pre_read_buf: Cursor<Vec<u8>>,
    inner_stream: Mutex<&'a mut dyn RunReadHalf>,
}

#[async_trait::async_trait]
impl<'a> RunReadHalf for PreReadStream<'a> {
    async fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // First, read from the pre-read buffer if it has anything left.
        if self.pre_read_buf.position() < self.pre_read_buf.get_ref().len() as u64 {
            let bytes_read = self.pre_read_buf.read(buf)?;
            if bytes_read > 0 {
                return Ok(bytes_read);
            }
        }
        // If the pre-read buffer is exhausted, read from the inner stream.
        let mut guard = self.inner_stream.lock().await;
        guard.read(buf).await
    }

    async fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let to_read = buf.len();
        let from_cursor = self.pre_read_buf.read(buf)?;

        if from_cursor < to_read {
            // The buffer is not filled yet. Read the rest from the inner stream.
            let mut guard = self.inner_stream.lock().await;
            guard.read_exact(&mut buf[from_cursor..]).await?;
        }

        Ok(to_read)
    }

    async fn handshake(&self) -> std::io::Result<Option<(RunAddr, String)>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "handshake not supported on PreReadStream",
        ))
    }
}

pub struct Htss5RunAcceptor {
    inner: Box<dyn RunAcceptor>,
    user: Option<String>,
    pw: Option<String>,
}

impl Htss5RunAcceptor {
    pub fn new(a: Box<dyn RunAcceptor>, user: Option<String>, pw: Option<String>) -> Self {
        Self {
            inner: a,
            user,
            pw,
        }
    }
}

#[async_trait::async_trait]
impl RunAcceptor for Htss5RunAcceptor {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        self.inner.accept().await
    }

    async fn handshake(
        &self,
        r: &mut dyn RunReadHalf,
        w: &mut dyn RunWriteHalf,
    ) -> std::io::Result<RunAddr> {
        let mut buf = [0u8; 2048];
        let n = r.read(&mut buf).await?;
        let data = buf[0..n].to_vec();

        if !data.is_empty() && data[0] == 0x05 {
            let mut pre_read_stream = PreReadStream {
                pre_read_buf: Cursor::new(data),
                inner_stream: Mutex::new(r),
            };

            let hello =
                &util::socks5::client_hello::ClientHello::parse(&mut pre_read_stream).await?;
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
            let req = &util::socks5::request::Request::parse(&mut pre_read_stream).await?;
            return req.try_into();
        }

        let mut cache = Some(data.clone());
        let str = String::from_utf8(data).unwrap();
        let lines = str.split("\r\n").collect::<Vec<&str>>();
        let f_line = lines[0];
        let parts = f_line.split(" ").collect::<Vec<&str>>();
        if parts.len() < 2 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                "invalid parts ".to_owned() + f_line,
            ));
        }
        let mut dst = parts[1].to_string();
        if !dst.contains("://") {
            dst = format!("http://{}", dst)
        }
        let ip_port = Url::parse(dst.as_str()).unwrap();
        if f_line.starts_with("CONNECT") {
            cache = None;
            w.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
            debug!("http HTTP/1.1 200 Connection Established\r\n\r\n");
        }

        Ok(RunAddr {
            addr: ip_port.host_str().unwrap().to_owned(),
            port: ip_port.port_or_known_default().unwrap(),
            udp: false,
            cache,
        })
    }
}
use crate::def::{RunAccStream, RunAcceptor, RunStream};
use crate::util::RunAddr;
use log::debug;
use std::io::ErrorKind;
use std::net::SocketAddr;
use url::Url;

#[allow(dead_code)]
pub struct HttpRunAcceptor {
    inner: Box<dyn RunAcceptor>,
    user: Option<String>,
    pw: Option<String>,
}

impl HttpRunAcceptor {
    pub fn new(a: Box<dyn RunAcceptor>, user: Option<String>, pw: Option<String>) -> Self {
        Self { inner: a, user, pw }
    }
}
#[async_trait::async_trait]
impl RunAcceptor for HttpRunAcceptor {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let res = self.inner.accept().await;
        res
    }

    async fn handshake(
        &self,
        stream: &mut dyn RunStream,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).await?;
        let data = buf[0..n].to_vec();
        let mut cache = Some(data.clone());
        let str = String::from_utf8(data).map_err(|e| std::io::Error::new(ErrorKind::Other, e))?;
        let lines = str.split("\r\n").collect::<Vec<&str>>();
        let f_line = lines.first().ok_or_else(|| {
            // 2. 使用 lines.first().ok_or_else 处理空数据
            std::io::Error::new(ErrorKind::InvalidData, "Empty request data")
        })?;
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
        let ip_port = Url::parse(dst.as_str()).map_err(|e| {
            std::io::Error::new(ErrorKind::InvalidData, format!("Invalid URL: {}", e))
        })?;
        if f_line.starts_with("CONNECT") {
            cache = None;
            stream
                .write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
            debug!("http HTTP/1.1 200 Connection Established\r\n\r\n");
        }

        Ok((
            RunAddr {
                addr: ip_port
                    .host_str()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidData, "URL has no host"))?
                    .to_owned(),
                port: ip_port.port_or_known_default().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidData, "URL has no port")
                })?,
                udp: false,
                // cache,
            },
            cache,
        ))
    }
}

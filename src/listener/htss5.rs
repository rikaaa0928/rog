use crate::def::{RunAccStream, RunAcceptor, RunStream};
use crate::util;
use crate::util::RunAddr;
use log::debug;
use std::io::ErrorKind;
use std::net::SocketAddr;
use url::Url;

#[allow(dead_code)]
pub struct Htss5RunAcceptor {
    inner: Box<dyn RunAcceptor>,
    user: Option<String>,
    pw: Option<String>,
}

impl Htss5RunAcceptor {
    pub fn new(a: Box<dyn RunAcceptor>, user: Option<String>, pw: Option<String>) -> Self {
        Self { inner: a, user, pw }
    }
}
#[async_trait::async_trait]
impl RunAcceptor for Htss5RunAcceptor {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let res = self.inner.accept().await;
        res
    }

    async fn handshake(
        &self,
        stream: &mut dyn RunStream,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        // stream.set_info(&mut |x| x.protocol_name = "http".to_string());
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no enough data",
            ));
        }
        let head1 = buf[0];
        if head1 == 5 {
            // socks5
            let mut data = buf[0..n].to_vec();
            let (hello, readed) = util::socks5::client_hello::ClientHello::parse_bytes(&data)?;

            if !hello.contains(util::socks5::NO_AUTH) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "no available authentication found",
                ));
            }
            let hello_back = util::socks5::server_hello::ServerHello::new(
                hello.version,
                util::socks5::NO_AUTH,
            );
            stream.write(&hello_back.to_bytes()).await?;
            let n = stream.read(&mut buf).await?;
            data = buf[0..n].to_vec();
            let (req, readed) = util::socks5::request::Request::parse_bytes(&data)?;
            let ret: std::io::Result<RunAddr> = (&req).try_into();
            data = data[readed..].to_vec();
            stream.set_info(&mut |x| x.protocol_name = "socks5".to_string());
            let cache = if data.is_empty() { None } else { Some(data) };
            match ret {
                Ok(addr) => Ok((addr, cache)),
                Err(e) => Err(e),
            }
        } else {
            // http
            let data = buf[0..n].to_vec();
            let mut cache = Some(data.clone());
            let str =
                String::from_utf8(data).map_err(|e| std::io::Error::other(e))?;
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
            stream.set_info(&mut |x| x.protocol_name = "http".to_string());
            Ok((
                RunAddr {
                    addr: ip_port
                        .host_str()
                        .ok_or_else(|| {
                            std::io::Error::new(ErrorKind::InvalidData, "URL has no host")
                        })?
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

    async fn post_handshake(
        &self,
        stream: &mut dyn RunStream,
        error: bool,
        port: u16,
    ) -> std::io::Result<()> {
        if stream.get_info().protocol_name != "socks5" {
            return Ok(());
        }
        let confirm = util::socks5::confirm::Confirm::new(error, port);
        stream.write(&confirm.to_bytes()).await?;
        Ok(())
    }
}

use crate::def::{RunAcceptor, RunReadHalf, RunStream, RunWriteHalf};
use crate::util::RunAddr;
use log::info;
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
    async fn accept(&self) -> std::io::Result<(Box<dyn RunStream>, SocketAddr)> {
        let res = self.inner.accept().await;
        res
    }

    async fn handshake(
        &self,
        r: &mut dyn RunReadHalf,
        w: &mut dyn RunWriteHalf,
    ) -> std::io::Result<RunAddr> {
        let mut buf = [0u8; 2048];
        let n = r.read(&mut buf).await?;
        let data = buf[0..n].to_vec();
        let mut cache = Some(data.clone());
        let str = String::from_utf8(data).unwrap();
        let lines = str.split("\r\n").collect::<Vec<&str>>();
        let f_line = lines[0];
        let parts = f_line.split(" ").collect::<Vec<&str>>();
        let mut dst = parts[1].to_string();
        if !dst.contains("://") {
            dst = format!("http://{}", dst)
        }
        let ip_port = Url::parse(dst.as_str()).unwrap();
        if f_line.starts_with("CONNECT") {
            cache = None;
            w.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
            info!("http HTTP/1.1 200 Connection Established\r\n\r\n");
        }

        Ok(RunAddr {
            addr: ip_port.host_str().unwrap().to_owned(),
            port: ip_port.port_or_known_default().unwrap(),
            udp: false,
            cache,
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

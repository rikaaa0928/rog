use crate::def::{RunAccStream, RunAcceptor, RunStream};
use crate::util::RunAddr;
use log::{debug, trace};
use std::io::ErrorKind;
use std::net::SocketAddr;
use url::Url;
use crate::consts::TCP_IO_BUFFER_SIZE;

#[allow(dead_code)]
pub struct HttpRunAcceptor {
    inner: Box<dyn RunAcceptor>,
    user: Option<String>,
    pw: Option<String>,
    server_id: String,
}

impl HttpRunAcceptor {
    pub fn new(
        a: Box<dyn RunAcceptor>,
        user: Option<String>,
        pw: Option<String>,
        server_id: String,
    ) -> Self {
        Self {
            inner: a,
            user,
            pw,
            server_id,
        }
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
        stream.set_info(&mut |x| x.protocol_name = "http".to_string());
        
        let mut buf = [0u8; TCP_IO_BUFFER_SIZE];
        let n = stream.read(&mut buf).await?;
        let data = buf[0..n].to_vec();

        let (header_str, body_bytes) = if let Some(idx) = data
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            let (h, b) = data.split_at(idx + 4);
            let s = String::from_utf8(h.to_vec()).map_err(std::io::Error::other)?;
            (s, Some(b.to_vec()))
        } else {
            let s = String::from_utf8(data.clone()).map_err(std::io::Error::other)?;
            (s, None)
        };

        let mut cache = Some(data);
        let str = header_str;
        let lines = str.split("\r\n").collect::<Vec<&str>>();
        let f_line = lines.first().ok_or_else(|| {
            // 2. 使用 lines.first().ok_or_else 处理空数据
            std::io::Error::new(ErrorKind::InvalidData, "Empty request data")
        })?;
        trace!("http first line: {}", f_line);
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
        } else {
            // Check loop
            for line in &lines {
                if line.to_lowercase().starts_with("via:") {
                    if line.contains(&self.server_id) {
                        return Err(std::io::Error::new(
                            ErrorKind::Other,
                            "Loop detected",
                        ));
                    }
                }
            }
            // Add Via header
            // Use str (which is valid String) instead of data (which was moved)
            if let Some(idx) = str.find("\r\n") {
                let (first_line, rest) = str.split_at(idx + 2); // +2 for \r\n
                let new_header = format!("{}Via: 1.1 {}\r\n{}", first_line, self.server_id, rest);
                let mut new_bytes = new_header.into_bytes();
                if let Some(mut b) = body_bytes {
                    new_bytes.append(&mut b);
                }
                cache = Some(new_bytes);
            }
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

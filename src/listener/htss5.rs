use crate::def::{RunAccStream, RunAcceptor, RunStream};
use crate::util;
use crate::util::RunAddr;
use log::{debug, info, trace};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::sleep;
use url::Url;

#[allow(dead_code)]
pub struct Htss5RunAcceptor {
    inner: Box<dyn RunAcceptor>,
    user: Option<String>,
    pw: Option<String>,
    ip_stats: Option<Arc<Mutex<HashMap<String, u64>>>>,
    server_id: String,
}

impl Htss5RunAcceptor {
    pub fn new(
        a: Box<dyn RunAcceptor>,
        user: Option<String>,
        pw: Option<String>,
        interval: Option<u64>,
        server_id: String,
    ) -> Self {
        let ip_stats = if let Some(i) = interval {
            if i > 0 {
                let stats: Arc<Mutex<HashMap<String, u64>>> =
                    Arc::new(Mutex::new(HashMap::new()));
                let stats_clone = stats.clone();
                tokio::spawn(async move {
                    loop {
                        sleep(Duration::from_secs(i)).await;
                        let mut map = stats_clone.lock().unwrap();
                        if map.is_empty() {
                            continue;
                        }
                        let mut count_vec: Vec<_> = map.iter().collect();
                        count_vec.sort_by(|a, b| b.1.cmp(a.1));
                        info!("--- IP Stats ({i}s) ---");
                        for (ip, count) in count_vec {
                            info!("{}: {}", ip, count);
                        }
                        info!("---------------------");
                        map.clear();
                    }
                });
                Some(stats)
            } else {
                None
            }
        } else {
            None
        };
        Self {
            inner: a,
            user,
            pw,
            ip_stats,
            server_id,
        }
    }
}
#[async_trait::async_trait]
impl RunAcceptor for Htss5RunAcceptor {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let res = self.inner.accept().await;
        if let Ok((_, addr)) = &res {
            if let Some(stats) = &self.ip_stats {
                let ip = addr.ip().to_string();
                let mut map = stats.lock().unwrap();
                *map.entry(ip).or_insert(0) += 1;
            }
        }
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
            let (hello, _readed) = util::socks5::client_hello::ClientHello::parse_bytes(&data)?;

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
                String::from_utf8(data).map_err(std::io::Error::other)?;
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

                if let Some(idx) = str.find("\r\n") {
                     let (first_line, rest) = str.split_at(idx + 2); // +2 for \r\n
                     let new_req = format!("{}Via: 1.1 {}\r\n{}", first_line, self.server_id, rest);
                     cache = Some(new_req.into_bytes());
                }
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

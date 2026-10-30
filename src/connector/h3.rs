use bytes::Bytes;
use log::{debug, error, info, warn};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use std::io::{self, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;

use crate::connector::grpc::parse_address;
use crate::def::{
    RunConnector, RunStream, RunUdpReader, RunUdpWriter, StreamInfo, config,
};
use crate::stream::h3_client::{
    ClientRecvBody, ClientSendBody, H3ClientRunStream, H3UdpClientRunReader, H3UdpClientRunWriter,
};
use crate::util::grpc_transport::GrpcTransportOptions;
use crate::util::h3_frame::{
    H3_RAW_STREAM_PATH, H3_RAW_UDP_PATH, encode_tcp_handshake_frame, encode_udp_handshake_frame,
};

const DEFAULT_H3_STREAM_WINDOW: u32 = 8 * 1024 * 1024;
const DEFAULT_H3_CONN_WINDOW: u32 = 16 * 1024 * 1024;
const DEFAULT_H2_MAX_FRAME_SIZE: u32 = 64 * 1024;

#[derive(Debug)]
struct NoCertVerification;

impl ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Clone)]
enum ActiveClientConn {
    H3(h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>),
    H2(h2::client::SendRequest<Bytes>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransportScheme {
    HttpsOrH3,
    H3Insecure,
    HttpCleartext,
}

pub struct H3RunConnector {
    host: String,
    port: u16,
    authority: String,
    scheme: TransportScheme,
    pw: String,
    fallback_h2: bool,
    connect_timeout: Duration,
    stream_window: u32,
    conn_window: u32,
    grpc_opts: GrpcTransportOptions,
    bootstrap_dns: Option<Vec<SocketAddr>>,
    quinn_endpoint_v4: Option<quinn::Endpoint>,
    quinn_endpoint_v6: Option<quinn::Endpoint>,
    tls_client_config_h2: Arc<rustls::ClientConfig>,
    pool: Vec<Arc<Mutex<Option<ActiveClientConn>>>>,
    rr_index: AtomicUsize,
}

impl H3RunConnector {
    pub async fn new(cfg: &config::Connector) -> io::Result<Self> {
        let endpoint_str = cfg.endpoint.as_ref().ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidInput,
                "h3 connector config is missing 'endpoint'",
            )
        })?;
        let pw = cfg.pw.clone().ok_or_else(|| {
            io::Error::new(
                ErrorKind::InvalidInput,
                "h3 connector config is missing 'pw'",
            )
        })?;

        let (scheme, normalized_url) = if let Some(rest) = endpoint_str.strip_prefix("h3://") {
            (TransportScheme::HttpsOrH3, format!("https://{rest}"))
        } else if let Some(rest) = endpoint_str.strip_prefix("h3c://") {
            (TransportScheme::H3Insecure, format!("https://{rest}"))
        } else if endpoint_str.starts_with("http://") {
            (TransportScheme::HttpCleartext, endpoint_str.clone())
        } else if endpoint_str.starts_with("https://") {
            (TransportScheme::HttpsOrH3, endpoint_str.clone())
        } else {
            (
                TransportScheme::HttpsOrH3,
                format!("https://{endpoint_str}"),
            )
        };

        let parsed_url = url::Url::parse(&normalized_url).map_err(|e| {
            io::Error::new(
                ErrorKind::InvalidInput,
                format!("invalid h3 endpoint '{}': {}", endpoint_str, e),
            )
        })?;
        let host = parsed_url
            .host_str()
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "endpoint missing host"))?
            .trim_matches(['[', ']'])
            .to_string();
        let port = parsed_url.port_or_known_default().unwrap_or(443);
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };

        let bootstrap_dns = crate::util::bootstrap::servers(&cfg.options)?;
        let grpc_opts = GrpcTransportOptions::from_options(&cfg.options);

        let insecure = scheme == TransportScheme::H3Insecure
            || cfg
                .options
                .as_ref()
                .and_then(|o| o.get("insecure").or_else(|| o.get("skip_cert_verify")))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

        let fallback_h2 = cfg
            .options
            .as_ref()
            .and_then(|o| o.get("fallback_h2"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let use_bbr = cfg
            .options
            .as_ref()
            .and_then(|o| o.get("congestion_control"))
            .and_then(|v| v.as_str())
            .map(|s| !s.eq_ignore_ascii_case("cubic"))
            .unwrap_or(true);

        let pool_size = cfg
            .options
            .as_ref()
            .and_then(|o| o.get("connections"))
            .and_then(|v| v.as_integer())
            .map(|n| (n as usize).clamp(1, 16))
            .unwrap_or(2);

        let stream_window = grpc_opts
            .initial_stream_window_size
            .unwrap_or(DEFAULT_H3_STREAM_WINDOW);
        let conn_window = grpc_opts
            .initial_connection_window_size
            .unwrap_or(DEFAULT_H3_CONN_WINDOW);
        let connect_timeout = grpc_opts
            .connect_timeout
            .unwrap_or(Duration::from_secs(10));

        let ca_pem_path = cfg
            .options
            .as_ref()
            .and_then(|o| o.get("ca_cert").or_else(|| o.get("ca_path")))
            .and_then(|v| v.as_str())
            .map(str::to_owned);

        let tls_h3 = Self::build_rustls_client_config(insecure, ca_pem_path.as_deref(), vec![b"h3".to_vec()])?;
        let tls_h2 = Arc::new(Self::build_rustls_client_config(
            insecure,
            ca_pem_path.as_deref(),
            vec![b"h2".to_vec()],
        )?);

        let (quinn_endpoint_v4, quinn_endpoint_v6) = if scheme != TransportScheme::HttpCleartext {
            let quic_tls =
                quinn::crypto::rustls::QuicClientConfig::try_from(tls_h3).map_err(|e| {
                    io::Error::new(ErrorKind::InvalidInput, format!("quic tls error: {e}"))
                })?;
            let mut client_cfg = quinn::ClientConfig::new(Arc::new(quic_tls));
            let mut transport = quinn::TransportConfig::default();
            let idle_timeout = grpc_opts
                .max_idle_timeout
                .unwrap_or(Duration::from_secs(60));
            transport.max_idle_timeout(idle_timeout.try_into().ok());
            transport.keep_alive_interval(grpc_opts.effective_keep_alive_interval());
            transport.receive_window(quinn::VarInt::from_u32(conn_window));
            transport.stream_receive_window(quinn::VarInt::from_u32(stream_window));
            transport.send_window(u64::from(conn_window));
            if use_bbr {
                transport.congestion_controller_factory(Arc::new(
                    quinn::congestion::BbrConfig::default(),
                ));
            }
            client_cfg.transport_config(Arc::new(transport));

            let ep_v4 = quinn::Endpoint::client(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                0,
            ))
            .ok()
            .map(|mut ep| {
                ep.set_default_client_config(client_cfg.clone());
                ep
            });

            let ep_v6 = quinn::Endpoint::client(SocketAddr::new(
                IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                0,
            ))
            .ok()
            .map(|mut ep| {
                ep.set_default_client_config(client_cfg);
                ep
            });

            (ep_v4, ep_v6)
        } else {
            (None, None)
        };

        let mut pool = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            pool.push(Arc::new(Mutex::new(None)));
        }

        Ok(Self {
            host,
            port,
            authority,
            scheme,
            pw,
            fallback_h2,
            connect_timeout,
            stream_window,
            conn_window,
            grpc_opts,
            bootstrap_dns,
            quinn_endpoint_v4,
            quinn_endpoint_v6,
            tls_client_config_h2: tls_h2,
            pool,
            rr_index: AtomicUsize::new(0),
        })
    }

    fn build_rustls_client_config(
        insecure: bool,
        ca_pem_path: Option<&str>,
        alpn: Vec<Vec<u8>>,
    ) -> io::Result<rustls::ClientConfig> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut cfg = if insecure {
            rustls::ClientConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e.to_string()))?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoCertVerification))
                .with_no_client_auth()
        } else {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            if let Some(path) = ca_pem_path {
                let pem_bytes = std::fs::read(path)?;
                let mut reader = std::io::BufReader::new(pem_bytes.as_slice());
                for cert in rustls_pemfile::certs(&mut reader) {
                    let cert = cert.map_err(|e| {
                        io::Error::new(ErrorKind::InvalidData, format!("invalid CA cert: {e}"))
                    })?;
                    roots.add(cert).map_err(|e| {
                        io::Error::new(ErrorKind::InvalidData, format!("add CA cert error: {e}"))
                    })?;
                }
            }
            rustls::ClientConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
                .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e.to_string()))?
                .with_root_certificates(roots)
                .with_no_client_auth()
        };
        cfg.alpn_protocols = alpn;
        Ok(cfg)
    }

    async fn resolve_target_addrs(&self) -> io::Result<Vec<SocketAddr>> {
        if let Some(ref servers) = self.bootstrap_dns {
            crate::util::bootstrap::resolve(&self.host, self.port, servers).await
        } else if let Ok(ip) = self.host.parse::<IpAddr>() {
            Ok(vec![SocketAddr::new(ip, self.port)])
        } else {
            let lookup = format!("{}:{}", self.host, self.port);
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host(lookup).await?.collect();
            if addrs.is_empty() {
                return Err(io::Error::new(
                    ErrorKind::NotFound,
                    format!("failed to resolve '{}'", self.host),
                ));
            }
            Ok(addrs)
        }
    }

    async fn establish_quinn_h3(&self, addrs: &[SocketAddr]) -> io::Result<ActiveClientConn> {
        let mut last_err = None;
        for &remote_addr in addrs {
            let ep = if remote_addr.is_ipv4() {
                self.quinn_endpoint_v4.as_ref()
            } else {
                self.quinn_endpoint_v6.as_ref()
            };
            let Some(endpoint) = ep else {
                continue;
            };
            let connecting = match endpoint.connect(remote_addr, &self.host) {
                Ok(c) => c,
                Err(e) => {
                    last_err = Some(io::Error::other(format!("quinn connect init error: {e}")));
                    continue;
                }
            };
            match tokio::time::timeout(self.connect_timeout, connecting).await {
                Ok(Ok(quinn_conn)) => {
                    let h3_quinn_conn = h3_quinn::Connection::new(quinn_conn);
                    let (mut driver, send_request) =
                        h3::client::new(h3_quinn_conn).await.map_err(|e| {
                            io::Error::other(format!("h3 client handshake error: {e}"))
                        })?;
                    tokio::spawn(async move {
                        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
                    });
                    info!(
                        "H3 (QUIC) connection established to {} ({})",
                        self.authority, remote_addr
                    );
                    return Ok(ActiveClientConn::H3(send_request));
                }
                Ok(Err(e)) => {
                    last_err = Some(io::Error::other(format!(
                        "quinn handshake to {} failed: {e}",
                        remote_addr
                    )));
                }
                Err(_) => {
                    last_err = Some(io::Error::new(
                        ErrorKind::TimedOut,
                        format!("quinn handshake to {} timed out", remote_addr),
                    ));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("no usable address for QUIC H3")))
    }

    async fn establish_h2(&self, addrs: &[SocketAddr], use_tls: bool) -> io::Result<ActiveClientConn> {
        let mut last_err = None;
        for &remote_addr in addrs {
            match tokio::time::timeout(self.connect_timeout, tokio::net::TcpStream::connect(remote_addr)).await {
                Ok(Ok(tcp)) => {
                    let _ = tcp.set_nodelay(self.grpc_opts.tcp_nodelay);
                    if let Some(tcp_keepalive) = self.grpc_opts.tcp_keepalive {
                        let _ = crate::util::grpc_transport::apply_tcp_keepalive(
                            &tcp,
                            tcp_keepalive,
                            self.grpc_opts.tcp_keepalive_interval,
                            self.grpc_opts.tcp_keepalive_retries,
                        );
                    }
                    let mut h2_builder = h2::client::Builder::new();
                    h2_builder
                        .initial_window_size(self.stream_window)
                        .initial_connection_window_size(self.conn_window)
                        .max_frame_size(DEFAULT_H2_MAX_FRAME_SIZE);

                    if use_tls {
                        let connector =
                            tokio_rustls::TlsConnector::from(self.tls_client_config_h2.clone());
                        let server_name = ServerName::try_from(self.host.clone()).map_err(|_| {
                            io::Error::new(ErrorKind::InvalidInput, "invalid TLS server name")
                        })?;
                        let tls_stream = tokio::time::timeout(
                            self.connect_timeout,
                            connector.connect(server_name, tcp),
                        )
                        .await
                        .map_err(|_| io::Error::new(ErrorKind::TimedOut, "TLS handshake timeout"))?
                        .map_err(|e| io::Error::other(format!("TLS handshake error: {e}")))?;

                        let (sender, conn) = h2_builder
                            .handshake(tls_stream)
                            .await
                            .map_err(|e| io::Error::other(format!("h2 handshake error: {e}")))?;
                        tokio::spawn(async move {
                            if let Err(e) = conn.await {
                                debug!("H2 fallback TLS connection closed: {e}");
                            }
                        });
                        info!(
                            "H2 (TLS fallback) connection established to {} ({})",
                            self.authority, remote_addr
                        );
                        return Ok(ActiveClientConn::H2(sender));
                    } else {
                        let (sender, conn) = h2_builder
                            .handshake(tcp)
                            .await
                            .map_err(|e| io::Error::other(format!("h2c handshake error: {e}")))?;
                        tokio::spawn(async move {
                            if let Err(e) = conn.await {
                                debug!("H2c connection closed: {e}");
                            }
                        });
                        info!(
                            "H2c connection established to {} ({})",
                            self.authority, remote_addr
                        );
                        return Ok(ActiveClientConn::H2(sender));
                    }
                }
                Ok(Err(e)) => last_err = Some(e),
                Err(_) => {
                    last_err = Some(io::Error::new(
                        ErrorKind::TimedOut,
                        format!("TCP connect to {} timed out", remote_addr),
                    ));
                }
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("no usable address for H2")))
    }

    async fn create_new_connection(&self) -> io::Result<ActiveClientConn> {
        let addrs = self.resolve_target_addrs().await?;
        match self.scheme {
            TransportScheme::HttpCleartext => self.establish_h2(&addrs, false).await,
            TransportScheme::HttpsOrH3 | TransportScheme::H3Insecure => {
                match self.establish_quinn_h3(&addrs).await {
                    Ok(conn) => Ok(conn),
                    Err(h3_err) => {
                        if self.fallback_h2 {
                            warn!(
                                "QUIC H3 connection to {} failed ({}), falling back to HTTP/2 over TLS",
                                self.authority, h3_err
                            );
                            self.establish_h2(&addrs, true).await
                        } else {
                            Err(h3_err)
                        }
                    }
                }
            }
        }
    }

    fn build_post_request(&self, path: &str) -> io::Result<http::Request<()>> {
        let scheme_str = if self.scheme == TransportScheme::HttpCleartext {
            "http"
        } else {
            "https"
        };
        let uri = format!("{scheme_str}://{}{path}", self.authority);
        http::Request::builder()
            .method(http::Method::POST)
            .uri(uri)
            .header(http::header::CONTENT_TYPE, "application/grpc")
            .header(http::header::TE, "trailers")
            .body(())
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e.to_string()))
    }

    async fn try_open_on_conn(
        &self,
        conn: &mut ActiveClientConn,
        path: &str,
    ) -> io::Result<(ClientRecvBody, ClientSendBody)> {
        let req = self.build_post_request(path)?;
        match conn {
            ActiveClientConn::H3(sender) => {
                let stream = sender.send_request(req).await.map_err(|e| {
                    io::Error::new(
                        ErrorKind::ConnectionReset,
                        format!("h3 send_request failed: {e}"),
                    )
                })?;
                let (send, recv) = stream.split();
                Ok((
                    ClientRecvBody::H3 {
                        stream: recv,
                        headers_received: false,
                    },
                    ClientSendBody::new_h3(send),
                ))
            }
            ActiveClientConn::H2(sender) => {
                std::future::poll_fn(|cx| sender.poll_ready(cx))
                    .await
                    .map_err(|e| {
                        io::Error::new(
                            ErrorKind::ConnectionReset,
                            format!("h2 poll_ready failed: {e}"),
                        )
                    })?;
                let (resp_fut, send_stream) = sender.send_request(req, false).map_err(|e| {
                    io::Error::new(
                        ErrorKind::ConnectionReset,
                        format!("h2 send_request failed: {e}"),
                    )
                })?;
                Ok((
                    ClientRecvBody::H2 {
                        response_fut: Some(resp_fut),
                        body: None,
                    },
                    ClientSendBody::new_h2(send_stream),
                ))
            }
        }
    }

    async fn open_bidi_stream(&self, path: &str) -> io::Result<(ClientRecvBody, ClientSendBody)> {
        let idx = self.rr_index.fetch_add(1, Ordering::Relaxed) % self.pool.len();
        let slot = &self.pool[idx];

        let existing = { slot.lock().await.clone() };
        if let Some(mut conn) = existing {
            if let Ok(pair) = self.try_open_on_conn(&mut conn, path).await {
                return Ok(pair);
            }
        }

        let mut guard = slot.lock().await;
        if let Some(ref mut conn) = *guard {
            if let Ok(pair) = self.try_open_on_conn(conn, path).await {
                return Ok(pair);
            }
        }

        let mut fresh_conn = self.create_new_connection().await?;
        let pair = self.try_open_on_conn(&mut fresh_conn, path).await?;
        *guard = Some(fresh_conn);
        Ok(pair)
    }
}

#[async_trait::async_trait]
impl RunConnector for H3RunConnector {
    async fn connect(&self, addr: String) -> io::Result<Box<dyn RunStream>> {
        let (dst_host, dst_port) = parse_address(&addr)?;
        let (recv, mut send) = self.open_bidi_stream(H3_RAW_STREAM_PATH).await?;

        // Send 0-RTT TCP handshake frame immediately without waiting for server 200 OK headers
        let hs_frame = encode_tcp_handshake_frame(&self.pw, &dst_host, dst_port, &[])?;
        send.send_raw_bytes(hs_frame).await.map_err(|e| {
            error!("h3 connector failed to send TCP handshake frame: {e}");
            e
        })?;

        let info = StreamInfo {
            protocol_name: "h3".to_string(),
            dst_addr: Some(dst_host),
            dst_port: Some(dst_port),
            udp: Some(false),
            ..Default::default()
        };
        Ok(Box::new(H3ClientRunStream::new(recv, send, info)))
    }

    async fn udp_tunnel(
        &self,
        src_addr: String,
    ) -> io::Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        let (src_host, src_port) = parse_address(&src_addr)?;
        let (recv, mut send) = self.open_bidi_stream(H3_RAW_UDP_PATH).await?;

        let hs_frame = encode_udp_handshake_frame(&self.pw, &src_host, src_port)?;
        send.send_raw_bytes(hs_frame).await.map_err(|e| {
            error!("h3 connector failed to send UDP handshake frame: {e}");
            e
        })?;

        Ok(Some((
            Box::new(H3UdpClientRunReader::new(recv, src_host, src_port)),
            Box::new(H3UdpClientRunWriter::new(send)),
        )))
    }
}

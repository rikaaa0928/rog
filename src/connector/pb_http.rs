use crate::connector::grpc::parse_address;
use crate::def::{RunConnector, RunStream, RunUdpReader, RunUdpWriter, config};
use crate::proto::v1::pb::StreamReq;
use crate::stream::pb_http_client::PbHttpClientRunStream;
use crate::stream::pb_http_h3_client::PbHttpH3ClientRunStream;
use crate::stream::pb_http_h3_udp_client::{PbHttpH3UdpClientReader, PbHttpH3UdpClientWriter};
use crate::stream::pb_http_udp_client::{PbHttpUdpClientReader, PbHttpUdpClientWriter};
use crate::util::crypto::encrypt_field;
use crate::util::pb_http::{
    CONTENT_TYPE_PROTOBUF, H3ClientRecvStream, H3ClientSendStream, PbHttpOptions, PbHttpTransport,
    request_uri, send_h3_message, send_message,
};
use base64::Engine;
use bytes::Bytes;
use http::{Method, Request, StatusCode};
use log::{error, warn};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::{TcpStream, lookup_host};
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;
use url::Host;

pub struct PbHttpRunConnector {
    cfg: config::Connector,
    options: PbHttpOptions,
}

enum PbHttpOpenedStream {
    H2 {
        reader: h2::RecvStream,
        writer: h2::SendStream<Bytes>,
    },
    H3 {
        reader: H3ClientRecvStream,
        writer: H3ClientSendStream,
    },
}

#[derive(Debug)]
struct EndpointParts {
    connect_addr: String,
    authority: String,
    host: String,
    port: u16,
    server_name: String,
}

impl PbHttpRunConnector {
    pub fn new(cfg: &config::Connector) -> io::Result<Self> {
        cfg.endpoint.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http connector config is missing 'endpoint'",
            )
        })?;
        if cfg.pw.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http connector config is missing 'pw'",
            ));
        }
        Ok(Self {
            cfg: cfg.clone(),
            options: PbHttpOptions::from_options(&cfg.options)?,
        })
    }

    async fn open_stream(&self, path: &str) -> io::Result<PbHttpOpenedStream> {
        let mut last_err = None;
        for transport in &self.options.client_transports {
            match self.open_stream_with_transport(*transport, path).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    warn!("pb_http {} open {} failed: {}", transport.as_str(), path, e);
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http connector has no client transports",
            )
        }))
    }

    async fn open_stream_with_transport(
        &self,
        transport: PbHttpTransport,
        path: &str,
    ) -> io::Result<PbHttpOpenedStream> {
        let endpoint = self.cfg.endpoint.as_ref().unwrap();
        match transport {
            PbHttpTransport::H2c => self.open_h2_stream(endpoint, path, false).await,
            PbHttpTransport::H2 => self.open_h2_stream(endpoint, path, true).await,
            PbHttpTransport::H3 => self.open_h3_stream(endpoint, path).await,
        }
    }

    async fn open_h2_stream(
        &self,
        endpoint: &str,
        path: &str,
        tls: bool,
    ) -> io::Result<PbHttpOpenedStream> {
        let mut parts = parse_endpoint(endpoint, if tls { "https" } else { "http" })?;
        if tls {
            apply_tls_server_name(&mut parts, &self.options);
        }
        let tcp = TcpStream::connect(&parts.connect_addr).await?;
        let transport = if tls {
            PbHttpTransport::H2
        } else {
            PbHttpTransport::H2c
        };
        let mut builder = h2::client::Builder::new();
        self.options.apply_client_builder(&mut builder);

        if tls {
            let tls_cfg = build_tls_config(&self.options, b"h2")?;
            let server_name = server_name(&parts.server_name)?;
            let stream = TlsConnector::from(Arc::new(tls_cfg))
                .connect(server_name, tcp)
                .await
                .map_err(io::Error::other)?;
            let (client, connection) = builder.handshake(stream).await.map_err(io::Error::other)?;
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    warn!("pb_http h2 client connection error: {}", e);
                }
            });
            open_h2_request(client, &parts, path, transport).await
        } else {
            let (client, connection) = builder.handshake(tcp).await.map_err(io::Error::other)?;
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    warn!("pb_http h2c client connection error: {}", e);
                }
            });
            open_h2_request(client, &parts, path, transport).await
        }
    }

    async fn open_h3_stream(&self, endpoint: &str, path: &str) -> io::Result<PbHttpOpenedStream> {
        let mut parts = parse_endpoint(endpoint, "https")?;
        apply_tls_server_name(&mut parts, &self.options);
        let remote_addr = resolve_quic_addr(&parts).await?;
        let bind_addr: SocketAddr = self.options.h3_bind.parse().map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid pb_http h3_bind {:?}: {}", self.options.h3_bind, e),
            )
        })?;
        let mut endpoint = quinn::Endpoint::client(bind_addr)?;
        let tls_cfg = build_tls_config(&self.options, b"h3")?;
        let mut quic_cfg = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(tls_cfg).map_err(io::Error::other)?,
        ));
        quic_cfg.transport_config(Arc::new(self.options.quic_transport_config()?));
        endpoint.set_default_client_config(quic_cfg);

        let quinn_conn = endpoint
            .connect(remote_addr, &parts.server_name)
            .map_err(io::Error::other)?
            .await
            .map_err(io::Error::other)?;
        let mut h3_builder = h3::client::builder();
        self.options.apply_h3_client_builder(&mut h3_builder);
        let (mut conn, mut sender) = h3_builder
            .build(h3_quinn::Connection::new(quinn_conn))
            .await
            .map_err(io::Error::other)?;
        tokio::spawn(async move {
            let err = std::future::poll_fn(|cx| conn.poll_close(cx)).await;
            warn!("pb_http h3 client connection closed: {}", err);
            endpoint.wait_idle().await;
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri(request_uri("https", &parts.authority, path))
            .header("content-type", CONTENT_TYPE_PROTOBUF)
            .body(())
            .map_err(io::Error::other)?;
        let mut stream = sender
            .send_request(request)
            .await
            .map_err(io::Error::other)?;
        let response = stream.recv_response().await.map_err(io::Error::other)?;
        if response.status() != StatusCode::OK {
            return Err(io::Error::other(format!(
                "pb_http h3 server returned status {} for {}",
                response.status(),
                path
            )));
        }
        let (writer, reader) = stream.split();
        Ok(PbHttpOpenedStream::H3 { reader, writer })
    }
}

async fn open_h2_request(
    client: h2::client::SendRequest<Bytes>,
    parts: &EndpointParts,
    path: &str,
    transport: PbHttpTransport,
) -> io::Result<PbHttpOpenedStream> {
    let mut client = client.ready().await.map_err(io::Error::other)?;
    let scheme = if transport == PbHttpTransport::H2 {
        "https"
    } else {
        "http"
    };
    let request = Request::builder()
        .method(Method::POST)
        .uri(request_uri(scheme, &parts.authority, path))
        .header("content-type", CONTENT_TYPE_PROTOBUF)
        .body(())
        .map_err(io::Error::other)?;
    let (response, writer) = client
        .send_request(request, false)
        .map_err(io::Error::other)?;
    let response = response.await.map_err(io::Error::other)?;
    if response.status() != StatusCode::OK {
        return Err(io::Error::other(format!(
            "pb_http {} server returned status {} for {}",
            transport.as_str(),
            response.status(),
            path
        )));
    }
    Ok(PbHttpOpenedStream::H2 {
        reader: response.into_body(),
        writer,
    })
}

#[async_trait::async_trait]
impl RunConnector for PbHttpRunConnector {
    async fn connect(&self, addr: String) -> io::Result<Box<dyn RunStream>> {
        let (host, port) = parse_address(addr.as_str())?;
        let pw = self.cfg.pw.as_ref().unwrap();
        let path = self.options.stream_path();

        let opened = self.open_stream(&path).await.map_err(|e| {
            error!("pb_http failed to open stream request: {}", e);
            e
        })?;

        let auth_req = StreamReq {
            auth: encrypt_field(pw, pw)?,
            dst_addr: Some(encrypt_field(&host, pw)?),
            dst_port: Some(port as u32),
            ..Default::default()
        };

        match opened {
            PbHttpOpenedStream::H2 { reader, mut writer } => {
                send_message(&mut writer, &auth_req, self.options.send_chunk_size).await?;
                let mut stream = PbHttpClientRunStream::new(
                    reader,
                    writer,
                    pw.clone(),
                    port != 443,
                    self.options.clone(),
                );
                stream.set_info(&mut |x| {
                    x.protocol_name = "pb_http".to_string();
                    x.dst_port = Some(port);
                    x.dst_addr = Some(host.to_string());
                });
                Ok(Box::new(stream))
            }
            PbHttpOpenedStream::H3 { reader, mut writer } => {
                send_h3_message(&mut writer, &auth_req, self.options.send_chunk_size).await?;
                let mut stream = PbHttpH3ClientRunStream::new(
                    reader,
                    writer,
                    pw.clone(),
                    port != 443,
                    self.options.clone(),
                );
                stream.set_info(&mut |x| {
                    x.protocol_name = "pb_http".to_string();
                    x.dst_port = Some(port);
                    x.dst_addr = Some(host.to_string());
                });
                Ok(Box::new(stream))
            }
        }
    }

    async fn udp_tunnel(
        &self,
        _src_addr: String,
    ) -> io::Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        let pw = self.cfg.pw.as_ref().unwrap().clone();
        let path = self.options.udp_path();
        let opened = self.open_stream(&path).await.map_err(|e| {
            error!("pb_http failed to open udp request: {}", e);
            e
        })?;

        match opened {
            PbHttpOpenedStream::H2 { reader, writer } => {
                let reader = Arc::new(Mutex::new(reader));
                let writer = Arc::new(Mutex::new(writer));
                Ok(Some((
                    Box::new(PbHttpUdpClientReader::new(
                        Arc::clone(&reader),
                        pw.clone(),
                        self.options.clone(),
                    )) as Box<dyn RunUdpReader>,
                    Box::new(PbHttpUdpClientWriter::new(
                        Arc::clone(&writer),
                        pw.clone(),
                        pw,
                        self.options.clone(),
                    )) as Box<dyn RunUdpWriter>,
                )))
            }
            PbHttpOpenedStream::H3 { reader, writer } => {
                let reader = Arc::new(Mutex::new(reader));
                let writer = Arc::new(Mutex::new(writer));
                Ok(Some((
                    Box::new(PbHttpH3UdpClientReader::new(
                        Arc::clone(&reader),
                        pw.clone(),
                        self.options.clone(),
                    )) as Box<dyn RunUdpReader>,
                    Box::new(PbHttpH3UdpClientWriter::new(
                        Arc::clone(&writer),
                        pw.clone(),
                        pw,
                        self.options.clone(),
                    )) as Box<dyn RunUdpWriter>,
                )))
            }
        }
    }
}

fn parse_endpoint(endpoint: &str, default_scheme: &str) -> io::Result<EndpointParts> {
    let raw = endpoint.trim();
    if raw.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pb_http endpoint must not be empty",
        ));
    }
    let url = if raw.contains("://") {
        url::Url::parse(raw)
    } else {
        url::Url::parse(&format!("{}://{}", default_scheme, raw))
    }
    .map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid pb_http endpoint {:?}: {}", endpoint, e),
        )
    })?;
    let host = url.host().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("pb_http endpoint {:?} is missing host", endpoint),
        )
    })?;
    let host_string = match host {
        Host::Domain(v) => v.to_string(),
        Host::Ipv4(v) => v.to_string(),
        Host::Ipv6(v) => v.to_string(),
    };
    let port = url.port_or_known_default().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("pb_http endpoint {:?} is missing port", endpoint),
        )
    })?;
    let authority = authority(&host_string, port);
    Ok(EndpointParts {
        connect_addr: authority.clone(),
        authority,
        host: host_string.clone(),
        port,
        server_name: host_string,
    })
}

async fn resolve_quic_addr(parts: &EndpointParts) -> io::Result<SocketAddr> {
    if let Ok(ip) = parts.host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, parts.port));
    }
    let mut addrs = lookup_host((parts.host.as_str(), parts.port)).await?;
    addrs.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("pb_http failed to resolve {}", parts.authority),
        )
    })
}

fn authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]:{}", host, port)
    } else {
        format!("{}:{}", host, port)
    }
}

fn apply_tls_server_name(parts: &mut EndpointParts, options: &PbHttpOptions) {
    if let Some(name) = &options.tls_server_name {
        parts.server_name = name.clone();
    }
}

fn server_name(name: &str) -> io::Result<ServerName<'static>> {
    ServerName::try_from(name.to_string()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid pb_http tls_server_name {:?}", name),
        )
    })
}

fn build_tls_config(options: &PbHttpOptions, alpn: &[u8]) -> io::Result<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?;
    let mut cfg = if options.tls_insecure_skip_verify {
        builder
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new(provider))
            .with_no_client_auth()
    } else {
        builder
            .with_root_certificates(root_store(options)?)
            .with_no_client_auth()
    };
    cfg.alpn_protocols = vec![alpn.to_vec()];
    Ok(cfg)
}

fn root_store(options: &PbHttpOptions) -> io::Result<rustls::RootCertStore> {
    let mut roots =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = &options.tls_ca_cert_file {
        let data = std::fs::read(path)?;
        let certs = parse_certificates(&data)?;
        if certs.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "pb_http tls_ca_cert_file {:?} contains no certificates",
                    path
                ),
            ));
        }
        let (added, ignored) = roots.add_parsable_certificates(certs);
        if added == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "pb_http tls_ca_cert_file {:?} has no parsable certificates; ignored {}",
                    path, ignored
                ),
            ));
        }
    }
    Ok(roots)
}

fn parse_certificates(data: &[u8]) -> io::Result<Vec<CertificateDer<'static>>> {
    if data.starts_with(b"-----BEGIN") {
        parse_pem_certificates(data)
    } else {
        Ok(vec![CertificateDer::from(data.to_vec())])
    }
}

fn parse_pem_certificates(data: &[u8]) -> io::Result<Vec<CertificateDer<'static>>> {
    let text = std::str::from_utf8(data).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("pb_http CA PEM is not valid UTF-8: {}", e),
        )
    })?;
    let mut certs = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN CERTIFICATE-----") {
        rest = &rest[start + "-----BEGIN CERTIFICATE-----".len()..];
        let Some(end) = rest.find("-----END CERTIFICATE-----") else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_http CA PEM certificate is missing END marker",
            ));
        };
        let body = &rest[..end];
        let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("pb_http CA PEM base64 decode failed: {}", e),
                )
            })?;
        certs.push(CertificateDer::from(der));
        rest = &rest[end + "-----END CERTIFICATE-----".len()..];
    }
    Ok(certs)
}

#[derive(Debug)]
struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl SkipServerVerification {
    fn new(provider: Arc<rustls::crypto::CryptoProvider>) -> Arc<Self> {
        Arc::new(Self(provider))
    }
}

impl ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

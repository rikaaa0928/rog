use bytes::Bytes;
use log::{debug, info, trace, warn};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::io::{self, Error, ErrorKind};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;
use tokio::sync::mpsc::{self, Receiver};
use tokio::sync::Mutex;

use crate::def::{RunAccStream, RunAcceptor, RunListener, RunStream};
use crate::object::config::ObjectConfig;
use crate::stream::h3_server::{
    H3ServerRunStream, H3UdpServerReadHalf, H3UdpServerWriteHalf, ServerRecvBody, ServerSendBody,
};
use crate::util::RunAddr;
use crate::util::grpc_transport::GrpcTransportOptions;
use crate::util::h3_frame::{H3_RAW_STREAM_PATH, H3_RAW_UDP_PATH};
use crate::util::parse_socket_addr;

const DEFAULT_H3_STREAM_WINDOW: u32 = 8 * 1024 * 1024;
const DEFAULT_H3_CONN_WINDOW: u32 = 16 * 1024 * 1024;
const DEFAULT_H2_MAX_FRAME_SIZE: u32 = 64 * 1024;
const DEFAULT_MAX_CONCURRENT_STREAMS: u32 = 256;

const LEGACY_GRPC_STREAM_PATH: &str = "/moe.rikaaa0928.rog.RogService/stream";
const LEGACY_GRPC_UDP_PATH: &str = "/moe.rikaaa0928.rog.RogService/udp";

pub struct H3Listener {
    cfg: ObjectConfig,
}

impl H3Listener {
    pub fn new(cfg: ObjectConfig) -> Self {
        Self { cfg }
    }

    fn load_or_generate_cert(
        options: &Option<std::collections::HashMap<String, toml::Value>>,
    ) -> io::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        let cert_path = options
            .as_ref()
            .and_then(|o| o.get("cert_path").or_else(|| o.get("cert")))
            .and_then(|v| v.as_str());
        let key_path = options
            .as_ref()
            .and_then(|o| o.get("key_path").or_else(|| o.get("key")))
            .and_then(|v| v.as_str());

        if let (Some(cpath), Some(kpath)) = (cert_path, key_path) {
            let cert_pem = std::fs::read(cpath)?;
            let key_pem = std::fs::read(kpath)?;
            let mut cert_reader = std::io::BufReader::new(cert_pem.as_slice());
            let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| {
                    io::Error::new(ErrorKind::InvalidData, format!("invalid cert PEM: {e}"))
                })?;
            if certs.is_empty() {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    "no certificates found in cert_path",
                ));
            }
            let mut key_reader = std::io::BufReader::new(key_pem.as_slice());
            let key = rustls_pemfile::private_key(&mut key_reader)
                .map_err(|e| {
                    io::Error::new(ErrorKind::InvalidData, format!("invalid key PEM: {e}"))
                })?
                .ok_or_else(|| {
                    io::Error::new(ErrorKind::InvalidData, "no private key found in key_path")
                })?;
            return Ok((certs, key));
        }

        let certified = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ])
        .map_err(|e| io::Error::other(format!("failed to generate self-signed cert: {e}")))?;

        let cert_der = certified.cert.der().clone();
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certified.key_pair.serialize_der(),
        ));
        Ok((vec![cert_der], key_der))
    }

    fn build_quinn_server_config(
        options: &Option<std::collections::HashMap<String, toml::Value>>,
        grpc_opts: &GrpcTransportOptions,
    ) -> io::Result<quinn::ServerConfig> {
        let (certs, key) = Self::load_or_generate_cert(options)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut tls_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e.to_string()))?
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e.to_string()))?;

        tls_config.alpn_protocols = vec![b"h3".to_vec()];
        tls_config.max_early_data_size = u32::MAX;

        let quic_tls = quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e.to_string()))?;
        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_tls));

        let stream_win = grpc_opts
            .initial_stream_window_size
            .unwrap_or(DEFAULT_H3_STREAM_WINDOW);
        let conn_win = grpc_opts
            .initial_connection_window_size
            .unwrap_or(DEFAULT_H3_CONN_WINDOW);
        let max_streams = grpc_opts
            .max_concurrent_streams
            .unwrap_or(DEFAULT_MAX_CONCURRENT_STREAMS);

        let mut transport = quinn::TransportConfig::default();
        transport.max_idle_timeout(Some(
            Duration::from_secs(60)
                .try_into()
                .expect("valid idle timeout"),
        ));
        transport.keep_alive_interval(Some(Duration::from_secs(15)));
        transport.receive_window(quinn::VarInt::from_u32(conn_win));
        transport.stream_receive_window(quinn::VarInt::from_u32(stream_win));
        transport.send_window(u64::from(conn_win));
        transport.max_concurrent_bidi_streams(quinn::VarInt::from_u32(max_streams));
        transport.max_concurrent_uni_streams(quinn::VarInt::from_u32(max_streams));
        transport.congestion_controller_factory(Arc::new(
            quinn::congestion::BbrConfig::default(),
        ));

        server_config.transport_config(Arc::new(transport));
        Ok(server_config)
    }
}

type UdpAcceptedItem = ((H3UdpServerReadHalf, H3UdpServerWriteHalf), SocketAddr);

pub struct H3RunListener {
    tcp_receiver: Arc<Mutex<Receiver<(H3ServerRunStream, SocketAddr)>>>,
    udp_receiver: Arc<Mutex<Receiver<UdpAcceptedItem>>>,
    auth: String,
}

fn classify_request_path(path: &str) -> Option<(bool, bool)> {
    // Returns Some((is_udp, is_legacy_pb))
    match path {
        H3_RAW_STREAM_PATH => Some((false, false)),
        LEGACY_GRPC_STREAM_PATH => Some((false, true)),
        H3_RAW_UDP_PATH => Some((true, false)),
        LEGACY_GRPC_UDP_PATH => Some((true, true)),
        _ => None,
    }
}

#[async_trait::async_trait]
impl RunListener for H3Listener {
    async fn listen(&self, addr: &str) -> io::Result<Box<dyn RunAcceptor>> {
        let sock_addr = parse_socket_addr(addr)?;
        let grpc_opts = GrpcTransportOptions::from_options(&self.cfg.listener.options);
        let channel_size = grpc_opts.stream_channel_size_or(64);
        let auth = self.cfg.listener.pw.clone().unwrap_or_default();

        let (tcp_tx, tcp_rx) = mpsc::channel(channel_size);
        let (udp_tx, udp_rx) = mpsc::channel(channel_size);

        // 1. Bind TCP H2c listener (for raddy H3->H2c, Cloudflare H3->H2->raddy->H2c, and H2 fallback)
        let tcp_listener = tokio::net::TcpListener::bind(sock_addr).await?;
        let actual_addr = tcp_listener.local_addr()?;
        info!("H3 listener bound TCP (H2c) on {}", actual_addr);

        let stream_win = grpc_opts
            .initial_stream_window_size
            .unwrap_or(DEFAULT_H3_STREAM_WINDOW);
        let conn_win = grpc_opts
            .initial_connection_window_size
            .unwrap_or(DEFAULT_H3_CONN_WINDOW);
        let max_frame = grpc_opts
            .max_frame_size
            .unwrap_or(DEFAULT_H2_MAX_FRAME_SIZE);
        let max_streams = grpc_opts
            .max_concurrent_streams
            .unwrap_or(DEFAULT_MAX_CONCURRENT_STREAMS);

        let tcp_tx_h2 = tcp_tx.clone();
        let udp_tx_h2 = udp_tx.clone();
        let auth_h2 = auth.clone();

        tokio::spawn(async move {
            loop {
                let (socket, peer_addr) = match tcp_listener.accept().await {
                    Ok(pair) => pair,
                    Err(e) => {
                        warn!("H3 (H2c) TCP accept error: {e}");
                        continue;
                    }
                };
                let _ = socket.set_nodelay(true);
                let tcp_tx = tcp_tx_h2.clone();
                let udp_tx = udp_tx_h2.clone();
                let auth = auth_h2.clone();

                tokio::spawn(async move {
                    let mut h2_builder = h2::server::Builder::new();
                    h2_builder
                        .initial_window_size(stream_win)
                        .initial_connection_window_size(conn_win)
                        .max_frame_size(max_frame)
                        .max_concurrent_streams(max_streams);

                    let mut h2_conn = match h2_builder.handshake(socket).await {
                        Ok(c) => c,
                        Err(e) => {
                            trace!("H2c handshake error from {}: {e}", peer_addr);
                            return;
                        }
                    };

                    while let Some(res) = h2_conn.accept().await {
                        let (req, mut respond) = match res {
                            Ok(pair) => pair,
                            Err(e) => {
                                trace!("H2c stream accept error from {}: {e}", peer_addr);
                                break;
                            }
                        };

                        let path = req.uri().path().to_string();
                        let Some((is_udp, legacy_pb)) = classify_request_path(&path) else {
                            let resp = http::Response::builder().status(404).body(()).unwrap();
                            let _ = respond.send_response(resp, true);
                            continue;
                        };

                        let resp = http::Response::builder()
                            .status(200)
                            .header(http::header::CONTENT_TYPE, "application/grpc")
                            .body(())
                            .unwrap();

                        let send_stream = match respond.send_response(resp, false) {
                            Ok(s) => s,
                            Err(e) => {
                                debug!("Failed to send H2c 200 OK headers to {}: {e}", peer_addr);
                                continue;
                            }
                        };
                        let recv_stream = req.into_body();

                        if is_udp {
                            let r = H3UdpServerReadHalf::new(
                                ServerRecvBody::new_h2(recv_stream),
                                auth.clone(),
                                legacy_pb,
                            );
                            let w = H3UdpServerWriteHalf::new(
                                ServerSendBody::new_h2(send_stream),
                                legacy_pb,
                            );
                            let _ = udp_tx.send(((r, w), peer_addr)).await;
                        } else {
                            let stream = H3ServerRunStream::new(
                                ServerRecvBody::new_h2(recv_stream),
                                ServerSendBody::new_h2(send_stream),
                                legacy_pb,
                            );
                            let _ = tcp_tx.send((stream, peer_addr)).await;
                        }
                    }
                });
            }
        });

        // 2. Bind UDP Quinn HTTP/3 listener on the same address/port for direct QUIC connections
        match Self::build_quinn_server_config(&self.cfg.listener.options, &grpc_opts)
            .and_then(|server_cfg| quinn::Endpoint::server(server_cfg, actual_addr))
        {
            Ok(endpoint) => {
                info!("H3 listener bound UDP (QUIC) on {}", actual_addr);
                let tcp_tx_h3 = tcp_tx;
                let udp_tx_h3 = udp_tx;
                let auth_h3 = auth.clone();

                tokio::spawn(async move {
                    while let Some(incoming) = endpoint.accept().await {
                        let peer_addr = incoming.remote_address();
                        let tcp_tx = tcp_tx_h3.clone();
                        let udp_tx = udp_tx_h3.clone();
                        let auth = auth_h3.clone();

                        tokio::spawn(async move {
                            let quinn_conn = match incoming.await {
                                Ok(c) => c,
                                Err(e) => {
                                    trace!("Direct QUIC handshake error from {}: {e}", peer_addr);
                                    return;
                                }
                            };
                            let mut h3_conn: h3::server::Connection<
                                h3_quinn::Connection,
                                Bytes,
                            > = match h3::server::Connection::new(h3_quinn::Connection::new(
                                quinn_conn,
                            ))
                            .await
                            {
                                Ok(c) => c,
                                Err(e) => {
                                    trace!("Direct H3 init error from {}: {e}", peer_addr);
                                    return;
                                }
                            };

                            loop {
                                match h3_conn.accept().await {
                                    Ok(Some(resolver)) => {
                                        let tcp_tx = tcp_tx.clone();
                                        let udp_tx = udp_tx.clone();
                                        let auth = auth.clone();
                                        tokio::spawn(async move {
                                            let (req, mut stream) =
                                                match resolver.resolve_request().await {
                                                    Ok(pair) => pair,
                                                    Err(e) => {
                                                        trace!(
                                                            "Direct H3 resolve_request error: {e}"
                                                        );
                                                        return;
                                                    }
                                                };

                                            let path = req.uri().path().to_string();
                                            let Some((is_udp, legacy_pb)) =
                                                classify_request_path(&path)
                                            else {
                                                let resp = http::Response::builder()
                                                    .status(404)
                                                    .body(())
                                                    .unwrap();
                                                let _ = stream.send_response(resp).await;
                                                let _ = stream.finish().await;
                                                return;
                                            };

                                            let resp = http::Response::builder()
                                                .status(200)
                                                .header(
                                                    http::header::CONTENT_TYPE,
                                                    "application/grpc",
                                                )
                                                .body(())
                                                .unwrap();
                                            if let Err(e) = stream.send_response(resp).await {
                                                debug!(
                                                    "Failed to send H3 200 OK headers to {}: {e}",
                                                    peer_addr
                                                );
                                                return;
                                            }

                                            let (send_half, recv_half) = stream.split();
                                            if is_udp {
                                                let r = H3UdpServerReadHalf::new(
                                                    ServerRecvBody::new_h3(recv_half),
                                                    auth,
                                                    legacy_pb,
                                                );
                                                let w = H3UdpServerWriteHalf::new(
                                                    ServerSendBody::new_h3(send_half),
                                                    legacy_pb,
                                                );
                                                let _ = udp_tx.send(((r, w), peer_addr)).await;
                                            } else {
                                                let run_stream = H3ServerRunStream::new(
                                                    ServerRecvBody::new_h3(recv_half),
                                                    ServerSendBody::new_h3(send_half),
                                                    legacy_pb,
                                                );
                                                let _ = tcp_tx.send((run_stream, peer_addr)).await;
                                            }
                                        });
                                    }
                                    Ok(None) => break,
                                    Err(e) => {
                                        trace!("Direct H3 accept closed for {}: {e}", peer_addr);
                                        break;
                                    }
                                }
                            }
                        });
                    }
                });
            }
            Err(e) => {
                warn!(
                    "H3 listener could not bind UDP on {} ({}), continuing with TCP H2c only",
                    actual_addr, e
                );
            }
        }

        Ok(Box::new(H3RunListener {
            tcp_receiver: Arc::new(Mutex::new(tcp_rx)),
            udp_receiver: Arc::new(Mutex::new(udp_rx)),
            auth,
        }))
    }
}

#[async_trait::async_trait]
impl RunAcceptor for H3RunListener {
    async fn accept(&self) -> io::Result<(RunAccStream, SocketAddr)> {
        let mut tcp_rx = self.tcp_receiver.lock().await;
        let mut udp_rx = self.udp_receiver.lock().await;
        select! {
            r = udp_rx.recv() => {
                match r {
                    None => Err(Error::other("H3 UDP receiver closed")),
                    Some(((r, w), peer_addr)) => Ok((
                        RunAccStream::UDPSocket((Box::new(r), Box::new(w))),
                        peer_addr,
                    )),
                }
            }
            r = tcp_rx.recv() => {
                match r {
                    None => Err(Error::other("H3 TCP receiver closed")),
                    Some((stream, peer_addr)) => Ok((
                        RunAccStream::TCPStream(Box::new(stream)),
                        peer_addr,
                    )),
                }
            }
        }
    }

    async fn handshake(
        &self,
        stream: &mut dyn RunStream,
    ) -> io::Result<(RunAddr, Option<Vec<u8>>)> {
        stream.set_info(&mut |x| x.protocol_name = "h3".to_string());
        let stream = stream
            .as_any_mut()
            .downcast_mut::<H3ServerRunStream>()
            .ok_or_else(|| Error::other("invalid stream type for H3RunListener"))?;
        let (addr, auth, initial_payload) = stream.handshake().await?;
        if auth != self.auth {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "invalid H3 stream auth",
            ));
        }
        Ok((addr, initial_payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::h3::H3RunConnector;
    use crate::def::{RunConnector, UDPMeta, UDPPacket, config};
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_h3_quic_and_h2c_tcp_and_udp_roundtrip() {
        let listener_cfg = config::Listener {
            name: "test-h3".to_string(),
            proto: "h3".to_string(),
            endpoint: "127.0.0.1:0".to_string(),
            router: "default".to_string(),
            user: None,
            pw: Some("secret123".to_string()),
            options: None,
        };
        let obj_cfg = ObjectConfig {
            server_id: "test".to_string(),
            listener: listener_cfg,
            connector: HashMap::new(),
        };

        // Pick a port that is free on both TCP and UDP
        let probe_tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe_tcp.local_addr().unwrap().port();
        drop(probe_tcp);
        let listen_addr = format!("127.0.0.1:{port}");

        let h3_listener = H3Listener::new(obj_cfg);
        let acceptor = Arc::new(h3_listener.listen(&listen_addr).await.unwrap());

        let acc_clone = acceptor.clone();
        tokio::spawn(async move {
            while let Ok((acc_stream, _)) = acc_clone.accept().await {
                let acc_inner = acc_clone.clone();
                tokio::spawn(async move {
                    match acc_stream {
                        RunAccStream::TCPStream(mut s) => {
                            let (addr, init) = acc_inner.handshake(s.as_mut()).await.unwrap();
                            let mut buf = [0u8; 1024];
                            let n = if let Some(init_bytes) = init {
                                buf[..init_bytes.len()].copy_from_slice(&init_bytes);
                                init_bytes.len()
                            } else {
                                s.read(&mut buf).await.unwrap()
                            };
                            let reply = format!(
                                "ack:{}:{}:{}",
                                addr.addr,
                                addr.port,
                                std::str::from_utf8(&buf[..n]).unwrap()
                            );
                            s.write(reply.as_bytes()).await.unwrap();
                        }
                        RunAccStream::UDPSocket((mut r, w)) => {
                            while let Ok(mut pkt) = r.read().await {
                                pkt.data = [b"udp-echo:".as_slice(), &pkt.data].concat();
                                w.write(pkt).await.unwrap();
                            }
                        }
                    }
                });
            }
        });

        for endpoint in [
            format!("h3c://127.0.0.1:{port}"),
            format!("http://127.0.0.1:{port}"),
        ] {
            let conn_cfg = config::Connector {
                name: "out".to_string(),
                proto: "h3".to_string(),
                endpoint: Some(endpoint.clone()),
                user: None,
                pw: Some("secret123".to_string()),
                dns: None,
                options: None,
            };
            let connector = H3RunConnector::new(&conn_cfg).await.unwrap();

            // 1. Test TCP stream
            let mut tcp_stream = connector.connect("1.1.1.1:443".to_string()).await.unwrap();
            tcp_stream.write(b"ping-h3").await.unwrap();
            let mut rbuf = [0u8; 256];
            let n = tcp_stream.read(&mut rbuf).await.unwrap();
            assert_eq!(
                std::str::from_utf8(&rbuf[..n]).unwrap(),
                "ack:1.1.1.1:443:ping-h3",
                "failed TCP stream on {endpoint}"
            );

            // 2. Test UDP stream (including ATYP_SAME second packet)
            let (mut udp_r, udp_w) = connector
                .udp_tunnel("192.168.1.10:50000".to_string())
                .await
                .unwrap()
                .unwrap();

            for i in 1..=2 {
                let msg = format!("dgram-{i}");
                udp_w
                    .write(UDPPacket {
                        meta: UDPMeta {
                            src_addr: "192.168.1.10".to_string(),
                            src_port: 50000,
                            dst_addr: "8.8.8.8".to_string(),
                            dst_port: 53,
                        },
                        data: msg.as_bytes().to_vec(),
                    })
                    .await
                    .unwrap();
                let resp = udp_r.read().await.unwrap();
                assert_eq!(resp.meta.dst_addr, "8.8.8.8");
                assert_eq!(resp.meta.dst_port, 53);
                assert_eq!(
                    std::str::from_utf8(&resp.data).unwrap(),
                    format!("udp-echo:{msg}")
                );
            }
        }
    }
}

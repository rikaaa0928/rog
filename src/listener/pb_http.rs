use crate::def::{RunAccStream, RunAcceptor, RunListener, RunStream};
use crate::object::config::ObjectConfig;
use crate::stream::pb_http_server::PbHttpServerRunStream;
use crate::stream::pb_http_udp_server::{PbHttpUdpServerReader, PbHttpUdpServerWriter};
use crate::util::RunAddr;
use crate::util::pb_http::{CONTENT_TYPE_PROTOBUF, PbHttpOptions};
use bytes::Bytes;
use http::{Method, Response, StatusCode};
use log::{error, warn};
use std::io::Error;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, mpsc};
use tokio::{select, spawn};

pub struct PbHttpListener {
    cfg: ObjectConfig,
    options: PbHttpOptions,
}

impl PbHttpListener {
    pub fn new(cfg: ObjectConfig) -> std::io::Result<PbHttpListener> {
        let options = PbHttpOptions::from_options(&cfg.listener.options)?;
        Ok(PbHttpListener { cfg, options })
    }
}

type UdpPair = (h2::RecvStream, h2::SendStream<Bytes>, String, PbHttpOptions);

pub struct PbHttpRunAcceptor {
    stream_receiver: Arc<Mutex<Receiver<PbHttpServerRunStream>>>,
    udp_receiver: Arc<Mutex<Receiver<UdpPair>>>,
    auth: String,
}

#[async_trait::async_trait]
impl RunListener for PbHttpListener {
    async fn listen(&self, addr: &str) -> std::io::Result<Box<dyn RunAcceptor>> {
        let (stream_tx, stream_rx) =
            mpsc::channel::<PbHttpServerRunStream>(self.options.accept_channel_size);
        let (udp_tx, udp_rx) = mpsc::channel::<UdpPair>(self.options.accept_channel_size);
        let auth = self.cfg.listener.pw.as_ref().ok_or_else(|| {
            Error::new(
                std::io::ErrorKind::InvalidInput,
                "pb_http listener config is missing 'pw'",
            )
        })?;
        let bind_addr = addr.to_owned();
        let options = self.options.clone();
        let auth = auth.clone();

        spawn(async move {
            let listener = match TcpListener::bind(&bind_addr).await {
                Ok(l) => l,
                Err(e) => {
                    error!("pb_http listener bind error: {}", e);
                    return;
                }
            };

            loop {
                let (tcp_stream, addr) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        error!("pb_http listener accept error: {}", e);
                        continue;
                    }
                };
                let stream_tx = stream_tx.clone();
                let udp_tx = udp_tx.clone();
                let auth = auth.clone();
                let options = options.clone();
                spawn(async move {
                    let mut builder = h2::server::Builder::new();
                    options.apply_server_builder(&mut builder);
                    let mut conn = match builder.handshake::<_, Bytes>(tcp_stream).await {
                        Ok(c) => c,
                        Err(e) => {
                            warn!("pb_http h2 handshake from {} error: {}", addr, e);
                            return;
                        }
                    };

                    while let Some(request) = conn.accept().await {
                        let stream_tx = stream_tx.clone();
                        let udp_tx = udp_tx.clone();
                        let auth = auth.clone();
                        let options = options.clone();
                        match request {
                            Ok((req, respond)) => {
                                spawn(async move {
                                    if let Err(e) = handle_request(
                                        req, respond, stream_tx, udp_tx, auth, options,
                                    )
                                    .await
                                    {
                                        warn!("pb_http request error: {}", e);
                                    }
                                });
                            }
                            Err(e) => {
                                warn!("pb_http h2 accept stream error: {}", e);
                            }
                        }
                    }
                });
            }
        });

        Ok(Box::new(PbHttpRunAcceptor {
            stream_receiver: Arc::new(Mutex::new(stream_rx)),
            udp_receiver: Arc::new(Mutex::new(udp_rx)),
            auth: self.cfg.listener.pw.as_ref().unwrap().clone(),
        }))
    }
}

async fn handle_request(
    req: http::Request<h2::RecvStream>,
    mut respond: h2::server::SendResponse<Bytes>,
    stream_tx: mpsc::Sender<PbHttpServerRunStream>,
    udp_tx: mpsc::Sender<UdpPair>,
    auth: String,
    options: PbHttpOptions,
) -> std::io::Result<()> {
    if req.method() != Method::POST {
        send_empty_response(&mut respond, StatusCode::METHOD_NOT_ALLOWED)?;
        return Ok(());
    }
    if req
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.starts_with(CONTENT_TYPE_PROTOBUF))
    {
        send_empty_response(&mut respond, StatusCode::UNSUPPORTED_MEDIA_TYPE)?;
        return Ok(());
    }

    let path = req.uri().path().to_string();
    let stream_path = options.stream_path();
    let udp_path = options.udp_path();
    let body = req.into_body();
    if path == stream_path {
        let writer = send_streaming_response(&mut respond)?;
        let stream = PbHttpServerRunStream::new(body, writer, options);
        if stream_tx.send(stream).await.is_err() {
            return Err(Error::other("pb_http stream channel closed"));
        }
    } else if path == udp_path {
        let writer = send_streaming_response(&mut respond)?;
        if udp_tx.send((body, writer, auth, options)).await.is_err() {
            return Err(Error::other("pb_http udp channel closed"));
        }
    } else {
        send_empty_response(&mut respond, StatusCode::NOT_FOUND)?;
    }
    Ok(())
}

fn send_streaming_response(
    respond: &mut h2::server::SendResponse<Bytes>,
) -> std::io::Result<h2::SendStream<Bytes>> {
    let response = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", CONTENT_TYPE_PROTOBUF)
        .body(())
        .map_err(Error::other)?;
    respond.send_response(response, false).map_err(Error::other)
}

fn send_empty_response(
    respond: &mut h2::server::SendResponse<Bytes>,
    status: StatusCode,
) -> std::io::Result<()> {
    let response = Response::builder()
        .status(status)
        .body(())
        .map_err(Error::other)?;
    respond
        .send_response(response, true)
        .map(|_| ())
        .map_err(Error::other)
}

#[async_trait::async_trait]
impl RunAcceptor for PbHttpRunAcceptor {
    async fn accept(&self) -> std::io::Result<(RunAccStream, SocketAddr)> {
        let mut stream_receiver = self.stream_receiver.lock().await;
        let mut udp_receiver = self.udp_receiver.lock().await;
        select! {
            r = udp_receiver.recv() => {
                match r {
                    None => Err(Error::other("udp receiver closed")),
                    Some((reader, writer, pw, options)) => {
                        let reader = Arc::new(Mutex::new(reader));
                        let writer = Arc::new(Mutex::new(writer));
                        Ok((
                            RunAccStream::UDPSocket((
                                Box::new(PbHttpUdpServerReader::new(
                                    Arc::clone(&reader),
                                    pw.clone(),
                                    pw.clone(),
                                    options.clone(),
                                )),
                                Box::new(PbHttpUdpServerWriter::new(
                                    Arc::clone(&writer),
                                    pw,
                                    options,
                                )),
                            )),
                            "127.0.9.28:2809".parse().unwrap(),
                        ))
                    }
                }
            }
            r = stream_receiver.recv() => {
                match r {
                    None => Err(Error::other("stream receiver closed")),
                    Some(stream) => Ok((
                        RunAccStream::TCPStream(Box::new(stream)),
                        "127.0.0.1:2809".parse().unwrap(),
                    )),
                }
            }
        }
    }

    async fn handshake(
        &self,
        stream: &mut dyn RunStream,
    ) -> std::io::Result<(RunAddr, Option<Vec<u8>>)> {
        stream.set_info(&mut |x| x.protocol_name = "pb_http".to_string());
        let stream = stream
            .as_any_mut()
            .downcast_mut::<PbHttpServerRunStream>()
            .unwrap();
        match stream.handshake(&self.auth).await? {
            Some((addr, auth)) => {
                if auth != self.auth {
                    return Err(Error::other("invalid auth"));
                }
                Ok((addr, None))
            }
            None => Err(Error::other("handshake failed")),
        }
    }
}

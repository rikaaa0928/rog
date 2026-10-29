use crate::consts::TCP_IO_BUFFER_SIZE;
use crate::def::RunStream;
use crate::util::RunAddr;
use log::debug;
use proxy_observe::ObserveConnection;
use std::io::Result;
use tokio::select;
use tokio_util::sync::CancellationToken;

pub async fn handle_tcp_connection(
    addr: RunAddr,
    cache: Option<Vec<u8>>,
    client_stream: Box<dyn RunStream>,
    server_stream: Box<dyn RunStream>,
    observe: Option<ObserveConnection>,
) -> Result<()> {
    debug!("Post Handshake successful {:?}", addr);
    let (mut client_r, mut client_w) = client_stream.split();
    let (mut server_r, mut server_w) = server_stream.split();
    let cancel_token = CancellationToken::new();

    if let Some(c) = cache {
        client_w.write(c.as_slice()).await?;
        if let Some(observe) = &observe {
            observe.add_tx(c.len() as u64);
        }
    }

    debug!("start loop");
    let token_reader = cancel_token.clone();
    let observe_tx = observe.clone();
    let s2c = tokio::spawn(async move {
        let mut buf = vec![0u8; TCP_IO_BUFFER_SIZE];
        loop {
            select! {
                tn = server_r.read(&mut buf) => {
                    match tn {
                        Err(e) => {
                            debug!("Reader task s2c error: {:?}", e);
                            break;
                        }
                        Ok(0) => {
                            debug!("Reader task s2c: r.read() returned 0 (EOF)");
                            break;
                        }
                        Ok(n) => {
                            if let Err(e) = client_w.write(&buf[..n]).await {
                                debug!("Reader task s2c: w.write() error: {:?}", e);
                                break;
                            }
                            if let Some(observe) = &observe_tx {
                                observe.add_tx(n as u64);
                            }
                        }
                    }
                }
                _ = token_reader.cancelled() => {
                    debug!("Reader task s2c interrupted by cancellation.");
                    break;
                }
            }
        }
        debug!("Reader task s2c finished, cancelling.");
        token_reader.cancel();
    });

    let token_writer = cancel_token.clone();
    let observe_rx = observe.clone();
    let c2s = tokio::spawn(async move {
        let mut buf = vec![0u8; TCP_IO_BUFFER_SIZE];
        loop {
            select! {
                tn = client_r.read(&mut buf) => {
                    match tn {
                        Err(e) => {
                            debug!("Writer task c2s error: {:?}", e);
                            break;
                        }
                        Ok(0) => {
                            debug!("Writer task c2s: r.read() returned 0 (EOF)");
                            break;
                        }
                        Ok(n) => {
                            if let Err(e) = server_w.write(&buf[..n]).await {
                                debug!("Writer task c2s: w.write() error: {:?}", e);
                                break;
                            }
                            if let Some(observe) = &observe_rx {
                                observe.add_rx(n as u64);
                            }
                        }
                    }
                }
                _ = token_writer.cancelled() => {
                    debug!("Writer task c2s interrupted by cancellation.");
                    break;
                }
            }
        }
        debug!("Writer task c2s finished, cancelling.");
        token_writer.cancel();
    });

    if let Err(e) = s2c.await {
        debug!("Reader task s2c panicked or was cancelled: {:?}", e);
    }
    if let Err(e) = c2s.await {
        debug!("Writer task c2s panicked or was cancelled: {:?}", e);
    }
    debug!("end loop");
    Ok(())
}

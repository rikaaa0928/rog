use crate::def::RunStream;
use crate::util::RunAddr;
use log::debug;
use std::io::Result;
use tokio::select;
use tokio_util::sync::CancellationToken;

pub async fn handle_tcp_connection(
    addr: RunAddr,
    cache: Option<Vec<u8>>,
    client_stream: Box<dyn RunStream>,
    server_stream: Box<dyn RunStream>,
) -> Result<()> {
    debug!("Post Handshake successful {:?}", addr);
    let (mut tcp_r, mut tcp_w) = client_stream.split();
    let (mut r, mut w) = server_stream.split();
    let cancel_token = CancellationToken::new();

    if let Some(c) = cache {
        tcp_w.write(c.as_slice()).await?;
    }

    debug!("start loop");
    let token_reader = cancel_token.clone();
    let x = tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            select! {
                tn = r.read(&mut buf) => {
                    match tn {
                        Err(e) => {
                            debug!("Reader task (r -> tcp_w) error: {:?}", e);
                            break;
                        }
                        Ok(0) => {
                            debug!("Reader task (r -> tcp_w): r.read() returned 0 (EOF)");
                            break;
                        }
                        Ok(n) => {
                            if let Err(e) = tcp_w.write(&buf[..n]).await {
                                debug!("Reader task (r -> tcp_w): tcp_w.write() error: {:?}", e);
                                break;
                            }
                        }
                    }
                }
                _ = token_reader.cancelled() => {
                    debug!("Reader task (r -> tcp_w) interrupted by cancellation.");
                    break;
                }
            }
        }
        debug!("Reader task (r -> tcp_w) finished, cancelling.");
        token_reader.cancel();
    });

    let token_writer = cancel_token.clone();
    let y = tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            select! {
                tn = tcp_r.read(&mut buf) => {
                    match tn {
                        Err(e) => {
                            debug!("Writer task (tcp_r -> w) error: {:?}", e);
                            break;
                        }
                        Ok(0) => {
                            debug!("Writer task (tcp_r -> w): tcp_r.read() returned 0 (EOF)");
                            break;
                        }
                        Ok(n) => {
                            if let Err(e) = w.write(&buf[..n]).await {
                                debug!("Writer task (tcp_r -> w): w.write() error: {:?}", e);
                                break;
                            }
                        }
                    }
                }
                _ = token_writer.cancelled() => {
                    debug!("Writer task (tcp_r -> w) interrupted by cancellation.");
                    break;
                }
            }
        }
        debug!("Writer task (tcp_r -> w) finished, cancelling.");
        token_writer.cancel();
    });

    if let Err(e) = x.await {
        debug!(
            "Reader task (r -> tcp_w) panicked or was cancelled: {:?}",
            e
        );
    }
    if let Err(e) = y.await {
        debug!(
            "Writer task (tcp_r -> w) panicked or was cancelled: {:?}",
            e
        );
    }
    debug!("end loop");
    Ok(())
}

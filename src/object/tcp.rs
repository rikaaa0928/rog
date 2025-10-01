use crate::def::{RunReadHalf, RunStream, RunWriteHalf};
use crate::util::RunAddr;
use log::debug;
use std::io::{Error, ErrorKind, Result};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::select;

pub async fn handle_tcp_connection(
    mut r: Box<dyn RunReadHalf>,
    mut w: Box<dyn RunWriteHalf>,
    addr: RunAddr,
    cache: Option<Vec<u8>>,
    client_stream: Box<dyn RunStream>,
) -> Result<()>
{
    debug!("Post Handshake successful {:?}", addr);
    let (mut tcp_r, mut tcp_w) = client_stream.split();
    let shutdown_signal = Arc::new(Notify::new());
    if cache.is_some() {
        tcp_w
            .write(cache.unwrap().as_slice())
            .await?;
    }

    debug!("start loop");
    let shutdown_signal_reader_task = shutdown_signal.clone();
    let x = tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            match select! {
                tn = r.read(&mut buf) => tn,
                _ = shutdown_signal_reader_task.notified() => Err(Error::new(ErrorKind::Other, "Interrupted by other task")),
            } {
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
        debug!("Reader task (r -> tcp_w) finished, notifying.");
        shutdown_signal_reader_task.notify_waiters();
    });
    let shutdown_signal_writer_task = shutdown_signal.clone();
    let y = tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            match select! {
                tn = tcp_r.read(&mut buf) => tn,
                _ = shutdown_signal_writer_task.notified() => Err(Error::new(ErrorKind::Other, "Interrupted by other task")),
            } {
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
        debug!("Writer task (tcp_r -> w) finished, notifying.");
        shutdown_signal_writer_task.notify_waiters();
    });
    if let Err(e) = x.await {
        debug!("Reader task (r -> tcp_w) panicked or was cancelled: {:?}", e);
    }
    if let Err(e) = y.await {
        debug!("Writer task (tcp_r -> w) panicked or was cancelled: {:?}", e);
    }
    debug!("end loop");
    Ok(())
}

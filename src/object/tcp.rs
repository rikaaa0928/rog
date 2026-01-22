use crate::def::RunStream;
use crate::util::RunAddr;
use log::debug;
use std::io::{Error, Result};
use std::sync::Arc;
use tokio::select;
use tokio::sync::Notify;

pub async fn handle_tcp_connection(
    addr: RunAddr,
    cache: Option<Vec<u8>>,
    mut client_stream: Box<dyn RunStream>,
    mut server_stream: Box<dyn RunStream>,
) -> Result<()> {
    debug!("Post Handshake successful {:?}", addr);

    if let Some(c) = cache {
        client_stream.write(&c).await?;
    }

    #[cfg(target_os = "linux")]
    {
        let c_res = client_stream.into_tcp_stream();
        let s_res = server_stream.into_tcp_stream();
        match (c_res, s_res) {
            (Ok(c_tcp), Ok(s_tcp)) => {
                return linux_ext::handle_splice(c_tcp, s_tcp).await;
            }
            (c_res, s_res) => {
                client_stream = match c_res {
                    Ok(tcp) => Box::new(crate::stream::tcp::TcpRunStream::new(tcp)),
                    Err(boxed) => boxed,
                };
                server_stream = match s_res {
                    Ok(tcp) => Box::new(crate::stream::tcp::TcpRunStream::new(tcp)),
                    Err(boxed) => boxed,
                };
            }
        }
    }

    let (mut tcp_r, mut tcp_w) = client_stream.split();
    let (mut r, mut w) = server_stream.split();
    let shutdown_signal = Arc::new(Notify::new());

    debug!("start loop");
    let shutdown_signal_reader_task = shutdown_signal.clone();
    let x = tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            match select! {
                tn = r.read(&mut buf) => tn,
                _ = shutdown_signal_reader_task.notified() => Err(Error::other("Interrupted by other task")),
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
                _ = shutdown_signal_writer_task.notified() => Err(Error::other("Interrupted by other task")),
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

#[cfg(target_os = "linux")]
mod linux_ext {
    use super::*;
    use tokio::net::TcpStream;
    use std::os::unix::io::{AsRawFd, BorrowedFd};
    use nix::fcntl::{splice, SpliceFFlags};
    use nix::unistd::pipe;

    pub async fn handle_splice(
        client: TcpStream,
        server: TcpStream,
    ) -> Result<()> {
        let (mut cr, mut cw) = client.into_split();
        let (mut sr, mut sw) = server.into_split();

        let shutdown_signal = Arc::new(Notify::new());

        let shutdown_signal_1 = shutdown_signal.clone();
        tokio::spawn(async move {
            if let Err(e) = splice_copy(&mut sr, &mut cw).await {
               debug!("Splice server -> client error: {}", e);
            }
            shutdown_signal_1.notify_waiters();
        });

        let shutdown_signal_2 = shutdown_signal.clone();
        tokio::spawn(async move {
            if let Err(e) = splice_copy(&mut cr, &mut sw).await {
               debug!("Splice client -> server error: {}", e);
            }
            shutdown_signal_2.notify_waiters();
        });

        let _ = shutdown_signal.notified().await;
        Ok(())
    }

    async fn splice_copy(r: &mut tokio::net::tcp::OwnedReadHalf, w: &mut tokio::net::tcp::OwnedWriteHalf) -> Result<()> {
        let (pipe_r, pipe_w) = pipe().map_err(Error::from)?;

        loop {
            r.readable().await?;

            // AsFd is required for nix::splice.
            let r_fd = unsafe { BorrowedFd::borrow_raw(r.as_ref().as_raw_fd()) };
            let w_fd = unsafe { BorrowedFd::borrow_raw(w.as_ref().as_raw_fd()) };

            let res = splice(
                r_fd,
                None,
                &pipe_w,
                None,
                65536,
                SpliceFFlags::SPLICE_F_MOVE | SpliceFFlags::SPLICE_F_NONBLOCK,
            );

            let n = match res {
                Ok(n) => n,
                Err(nix::errno::Errno::EAGAIN) => continue,
                Err(e) => return Err(Error::from(e)),
            };

            if n == 0 {
                break;
            }

            let mut total_written = 0;
            while total_written < n {
                w.writable().await?;
                let res = splice(
                    &pipe_r,
                    None,
                    w_fd,
                    None,
                    n - total_written,
                    SpliceFFlags::SPLICE_F_MOVE | SpliceFFlags::SPLICE_F_NONBLOCK,
                );

                match res {
                    Ok(written) => total_written += written,
                    Err(nix::errno::Errno::EAGAIN) => continue,
                    Err(e) => return Err(Error::from(e)),
                }
            }
        }
        Ok(())
    }
}

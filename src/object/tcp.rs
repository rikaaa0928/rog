use crate::def::{RouterSet, RunConnector, RunReadHalf, RunStream, RunWriteHalf};
use crate::object::config::ObjectConfig;
use crate::util::RunAddr;
use log::debug;
use std::io::{Error, ErrorKind, Result};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{oneshot, Mutex};
use tokio::select;

pub async fn handle_tcp_connection(
    mut r: Box<dyn RunReadHalf>,
    mut w: Box<dyn RunWriteHalf>,
    addr: RunAddr,
    client_stream: Box<dyn RunStream>,
) -> Result<()>
{
    debug!("Post Handshake successful {:?}", addr);
    let (mut tcp_r, mut tcp_w) = client_stream.split();
    let (reader_interrupter, mut reader_interrupt_receiver) =
        oneshot::channel();
    let (writer_interrupter, mut writer_interrupt_receiver) =
        oneshot::channel();
    if addr.cache.is_some() {
        tcp_w
            .write(addr.cache.clone().unwrap().as_slice())
            .await?;
    }

    debug!("start loop");
    let x = tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            let reader_interrupt_receiver = &mut reader_interrupt_receiver;
            match select! {
                tn=r.read(&mut buf) => tn,
               _=reader_interrupt_receiver=>Err(Error::new(ErrorKind::Other, "Interrupted")),
            } {
                Err(e) => {
                    break;
                }
                Ok(0) => {
                    break;
                }
                Ok(n) => {
                    let res = tcp_w.write(&buf[..n]).await;
                    if res.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = writer_interrupter.send(());
    });
    let y = tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        loop {
            let writer_interrupt_receiver = &mut writer_interrupt_receiver;
            let n_res = select! {
                tn=tcp_r.read(&mut buf) => tn,
               _=writer_interrupt_receiver=>Err(Error::new(ErrorKind::Other, "Interrupted")),
            };
            if n_res.is_err() {
                break;
            }
            let n = n_res.unwrap();
            if n == 0 {
                break;
            }
            let res = w.write(&buf[..n]).await;
            if res.is_err() {
                break;
            }
        }
        let _ = reader_interrupter.send(());
    });
    let _ = x.await;
    let _ = y.await;
    debug!("end loop");
    Ok(())
}

use crate::block::{BlockManager, DataBlock};
use crate::consts::TCP_IO_BUFFER_SIZE;
use crate::def::RunStream;
use crate::util::RunAddr;
use bytes::Bytes;
use log::debug;
use std::io::Result;
use std::sync::Arc;
use tokio::select;
use tokio_util::sync::CancellationToken;

pub async fn handle_tcp_connection(
    addr: RunAddr,
    cache: Option<Vec<u8>>,
    client_stream: Box<dyn RunStream>,
    server_stream: Box<dyn RunStream>,
    block_manager: Option<Arc<BlockManager>>,
) -> Result<()> {
    debug!("Post Handshake successful {:?}", addr);
    let (mut client_r, mut client_w) = client_stream.split();
    let (mut server_r, mut server_w) = server_stream.split();
    let cancel_token = CancellationToken::new();

    if let Some(c) = cache {
        client_w.write(c.as_slice()).await?;
    }

    debug!("start loop");
    if let Some(block_manager) = block_manager {
        // s read
        let s2c_data_block = Arc::new(DataBlock::new(block_manager.clone()));
        let s2c_data_block_r = s2c_data_block.clone();
        let server_read_token = cancel_token.clone();
        let server_reader_task = tokio::spawn(async move {
            let mut buf = [0u8; TCP_IO_BUFFER_SIZE];
            loop {
                select! {
                    tn = server_r.read(&mut buf) => {
                        match tn {
                            Err(e) => {
                                debug!("Reader task server_reader error: {:?}", e);
                                break;
                            }
                            Ok(0) => {
                                debug!("Reader task server_reader: r.read() returned 0 (EOF)");
                                break;
                            }
                            Ok(n) => {
                                s2c_data_block.provide(Bytes::copy_from_slice(&buf[..n])).await;
                            }
                        }
                    }
                    _ = server_read_token.cancelled() => {
                        debug!("Reader task server_reader interrupted by cancellation.");
                        break;
                    }
                }
            }
            debug!("Reader task server_reader finished, cancelling.");
            server_read_token.cancel();
        });
        // c write
        let client_write_token = cancel_token.clone();
        let client_writer_task = tokio::spawn(async move {
            loop {
                select! {
                    data = s2c_data_block_r.consume() =>{
                        if let Err(e) = client_w.write(&data).await {
                                debug!("Reader task client_writer: w.write() error: {:?}", e);
                                break;
                            }
                    }
                    _ = client_write_token.cancelled() => {
                        debug!("Writer task client_writer interrupted by cancellation.");
                        break;
                    }
                }
            }
        });
        // c read
        let c2s_data_block = Arc::new(DataBlock::new(block_manager.clone()));
        let c2s_data_block_r = c2s_data_block.clone();
        let client_read_token = cancel_token.clone();
        let client_reader_task = tokio::spawn(async move {
            let mut buf = [0u8; TCP_IO_BUFFER_SIZE];
            loop {
                select! {
                     tn = client_r.read(&mut buf) => {
                        match tn {
                            Err(e) => {
                                debug!("Reader task client_reader error: {:?}", e);
                                break;
                            }
                            Ok(0) => {
                                debug!("Reader task client_reader: r.read() returned 0 (EOF)");
                                break;
                            }
                            Ok(n) => {
                                c2s_data_block.provide(Bytes::copy_from_slice(&buf[..n])).await;
                            }
                        }
                    }
                    _ = client_read_token.cancelled() => {
                        debug!("Reader task client_reader interrupted by cancellation.");
                        break;
                    }
                }
            }
             debug!("Reader task client_reader finished, cancelling.");
            client_read_token.cancel();
        });

        // s write
        let server_write_token = cancel_token.clone();
        let server_writer_task = tokio::spawn(async move {
             loop {
                select! {
                    data = c2s_data_block_r.consume() =>{
                        if let Err(e) = server_w.write(&data).await {
                                debug!("Reader task server_writer: w.write() error: {:?}", e);
                                break;
                            }
                    }
                     _ = server_write_token.cancelled() => {
                        debug!("Writer task server_writer interrupted by cancellation.");
                        break;
                    }
                }
            }
             debug!("Writer task server_writer finished, cancelling.");
            server_write_token.cancel();
        });

        // wait all
        let _ = server_reader_task.await;
        let _ = client_writer_task.await;
        let _ = client_reader_task.await;
        let _ = server_writer_task.await;

        return Ok(());
    }
    let token_reader = cancel_token.clone();
    let s2c = tokio::spawn(async move {
        let mut buf = [0u8; TCP_IO_BUFFER_SIZE];
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
    let c2s = tokio::spawn(async move {
        let mut buf = [0u8; TCP_IO_BUFFER_SIZE];
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
        debug!(
            "Reader task s2c panicked or was cancelled: {:?}",
            e
        );
    }
    if let Err(e) = c2s.await {
        debug!(
            "Writer task c2s panicked or was cancelled: {:?}",
            e
        );
    }
    debug!("end loop");
    Ok(())
}

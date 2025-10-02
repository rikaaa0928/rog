use crate::def::ReadWrite;
use crate::util::RunAddr;
use log::debug;
use std::io::Result;
use tokio::io::{copy_bidirectional, AsyncWriteExt};

pub async fn handle_tcp_connection(
    mut downstream: Box<dyn ReadWrite>,
    mut upstream: Box<dyn ReadWrite>,
    addr: RunAddr,
    cache: Option<Vec<u8>>,
) -> Result<()> {
    debug!("Handling TCP connection for {:?}", addr);

    if let Some(data) = cache {
        if !data.is_empty() {
            debug!("Writing {} bytes from cache to upstream", data.len());
            upstream.write_all(&data).await?;
        }
    }

    debug!("Starting bidirectional copy for {:?}", addr);
    match copy_bidirectional(&mut downstream, &mut upstream).await {
        Ok((up, down)) => {
            debug!(
                "TCP connection for {:?} finished: {} bytes uploaded, {} bytes downloaded",
                addr, up, down
            );
        }
        Err(e) => {
            debug!("TCP connection for {:?} ended with error: {}", addr, e);
        }
    }

    Ok(())
}
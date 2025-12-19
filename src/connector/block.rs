use crate::def::{RunConnector, RunStream, RunUdpReader, RunUdpWriter};
use std::io::{Error, ErrorKind, Result};

pub struct BlockRunConnector {}

impl BlockRunConnector {
    pub fn new() -> Self {
        BlockRunConnector {}
    }
}

#[async_trait::async_trait]
impl RunConnector for BlockRunConnector {
    async fn connect(&self, _addr: String) -> Result<Box<dyn RunStream>> {
        Err(Error::new(
            ErrorKind::PermissionDenied,
            "Connection blocked by rule",
        ))
    }

    async fn udp_tunnel(
        &self,
        _src_addr: String,
    ) -> Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        Err(Error::new(
            ErrorKind::PermissionDenied,
            "Connection blocked by rule",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_block_connector() {
        let connector = BlockRunConnector::new();
        let res = connector.connect("1.1.1.1:80".to_string()).await;
        assert!(res.is_err());
        assert_eq!(res.err().unwrap().kind(), ErrorKind::PermissionDenied);

        let res = connector.udp_tunnel("1.1.1.1:80".to_string()).await;
        assert!(res.is_err());
        assert_eq!(res.err().unwrap().kind(), ErrorKind::PermissionDenied);
    }
}

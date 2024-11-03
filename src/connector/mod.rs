use crate::connector::tcp::TcpRunConnector;
use crate::def::{config, RunConnector};

pub(crate) mod tcp;

pub async fn create(cfg: &config::Connector) -> std::io::Result<Box<dyn RunConnector>> {
    match cfg.proto.as_str() {
        "tcp" => {
            let res = TcpRunConnector::new();
            Ok(Box::new(res))
        }
        _ => {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("connector proto {} not found", cfg.proto.as_str())))
        }
    }
}
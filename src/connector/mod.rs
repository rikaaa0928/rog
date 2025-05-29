use crate::connector::grpc::GrpcRunConnector;
use crate::connector::tcp::TcpRunConnector;
use crate::connector::rogv2::RogV2Connector;
use crate::def::{config, RunConnector};

pub(crate) mod tcp;
pub(crate) mod grpc;
pub(crate) mod rogv2;

pub async fn create(cfg: &config::Connector) -> std::io::Result<Box<dyn RunConnector>> {
    match cfg.proto.as_str() {
        "tcp" => {
            let res = TcpRunConnector::new();
            Ok(Box::new(res))
        }
        "grpc" => {
            let res=GrpcRunConnector::new(cfg).await?;
            Ok(Box::new(res))
        }
        "rogv2" => {
            let res = RogV2Connector::new(cfg).await?;
            Ok(Box::new(res))
        }
        _ => {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("connector proto {} not found", cfg.proto.as_str())))
        }
    }
}

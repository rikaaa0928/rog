use crate::connector::grpc::GrpcRunConnector;
use crate::connector::pb_tcp::PbTcpRunConnector;
use crate::connector::rev_grpc::RevGrpcRunConnector;
use crate::connector::tcp::TcpRunConnector;
use crate::def::{RunConnector, config};

pub(crate) mod block;
pub(crate) mod grpc;
pub(crate) mod pb_tcp;
pub(crate) mod rev_grpc;
pub(crate) mod tcp;

pub async fn create(cfg: &config::Connector) -> std::io::Result<Box<dyn RunConnector>> {
    match cfg.proto.as_str() {
        "tcp" => {
            let res = TcpRunConnector::new();
            Ok(Box::new(res))
        }
        "grpc" => {
            let res = GrpcRunConnector::new(cfg).await?;
            Ok(Box::new(res))
        }
        "rev_grpc" => {
            let res = RevGrpcRunConnector::new(cfg).await?;
            Ok(Box::new(res))
        }
        "pb_tcp" => {
            let res = PbTcpRunConnector::new(cfg)?;
            Ok(Box::new(res))
        }
        "block" => {
            let res = crate::connector::block::BlockRunConnector::new();
            Ok(Box::new(res))
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("connector proto {} not found", cfg.proto.as_str()),
        )),
    }
}

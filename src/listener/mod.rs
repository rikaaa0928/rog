use crate::def::{RouterSet, RunAcceptor, RunListener};
use crate::listener::auto::AutoRunAcceptor;
use crate::listener::grpc::GrpcListener;
use crate::listener::http::HttpRunAcceptor;
use crate::listener::socks5::SocksRunAcceptor;
use crate::listener::tcp::TcpRunListener;
use crate::object::config::ObjectConfig;
use std::sync::Arc;

pub(crate) mod auto;
pub(crate) mod grpc;
pub(crate) mod http;
pub(crate) mod socks5;
pub(crate) mod tcp;

pub async fn create(
    cfg: &ObjectConfig,
    router: Arc<dyn RouterSet>,
) -> std::io::Result<Box<dyn RunAcceptor>> {
    match cfg.listener.proto.as_str() {
        "socks5" => {
            let listener = TcpRunListener {}
                .listen(cfg.listener.endpoint.as_str())
                .await?;
            let socks5 = Box::new(SocksRunAcceptor::new(listener, None, None));
            Ok(socks5)
        }
        "grpc" => {
            let grpc = GrpcListener::new(cfg.clone()); // router argument removed from call
            let acc = grpc.listen(cfg.listener.endpoint.as_str()).await?;
            Ok(acc)
        }
        "http" => {
            let listener = TcpRunListener {}
                .listen(cfg.listener.endpoint.as_str())
                .await?;
            let http = Box::new(HttpRunAcceptor::new(listener, None, None));
            Ok(http)
        }
        "auto" => {
            let listener = TcpRunListener {}
                .listen(cfg.listener.endpoint.as_str())
                .await?;
            let auto = Box::new(AutoRunAcceptor::new(listener));
            Ok(auto)
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("listener proto {} not found", cfg.listener.proto.as_str()),
        )),
    }
}

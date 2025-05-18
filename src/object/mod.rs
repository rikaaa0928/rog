use crate::def::{RouterSet, RunConnector};
use crate::object::config::ObjectConfig;
use crate::router::DefaultRouter;
use crate::util::RunAddr;
use crate::{connector, listener};
use log::{debug, error};
use std::io;
use std::io::{Error, ErrorKind, Result};
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::Mutex;

pub mod config;
pub mod tcp;
pub mod udp;

pub struct Object {
    config: Arc<ObjectConfig>,
    router: Arc<dyn RouterSet>,
}

impl Object {
    pub fn new(config: Arc<ObjectConfig>, router: Arc<dyn RouterSet>) -> Self {
        Self { config, router }
    }

    pub async fn start(&self) -> io::Result<()> {
        let config = self.config.clone();
        let router = self.router.clone();
        let acc = listener::create(&config, router.clone()).await?;
        let acc = Arc::new(acc);

        loop {
            let (s, _) = acc.accept().await?;
            let acc = Arc::clone(&acc);
            let router = Arc::clone(&router);
            let config = config.clone();
            spawn(async move {
                let (mut r, mut w) = s.split();
                // let udp_tunnel=Arc::new(&connector).lock().await.udp_tunnel();
                let addr_res = acc.handshake(r.as_mut(), w.as_mut()).await;
                match addr_res {
                    Err(e) => {
                        error!("Handshake error: {}", e);
                    }
                    Ok(addr) => {
                        let addr_ref = &addr;

                        if addr_ref.udp {
                            udp::handle_udp_connection(
                                r,
                                w,
                                Arc::clone(&acc),
                                config.clone(),
                                router.clone(),
                                addr,
                            )
                            .await?;
                        } else {
                            let client_name = router
                                .route(
                                    config.listener.name.as_str(),
                                    config.listener.router.as_str(),
                                    addr_ref,
                                )
                                .await;
                            let conn_conf = config.connector.get(client_name.as_str()).unwrap();
                            let connector_obj =
                                Arc::new(Mutex::new(connector::create(conn_conf).await?));
                            debug!("Handshake successful {:?}", addr_ref);
                            let client_stream_res = Arc::clone(&connector_obj)
                                .lock()
                                .await
                                .connect(addr_ref.endpoint())
                                .await;
                            let mut error = false;
                            if client_stream_res.is_err() {
                                error = true;
                            }
                            let client_stream = client_stream_res?;
                            acc.post_handshake(r.as_mut(), w.as_mut(), error, 0).await?;
                            tcp::handle_tcp_connection(r, w, addr, client_stream).await?;
                        }
                    }
                }
                Ok::<(), Error>(())
            });
        }
        Ok(())
    }
}

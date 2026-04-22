use crate::connector::grpc::parse_address;
use crate::def::{RunConnector, RunStream, RunUdpReader, RunUdpWriter, config};
use crate::proto::v1::pb::StreamReq;
use crate::stream::pb_tcp_client::PbTcpClientRunStream;
use crate::stream::pb_tcp_udp_client::{PbTcpUdpClientReader, PbTcpUdpClientWriter};
use crate::util::crypto::encrypt_field;
use crate::util::tcp_frame::*;
use std::io;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

pub struct PbTcpRunConnector {
    cfg: config::Connector,
}

impl PbTcpRunConnector {
    pub fn new(cfg: &config::Connector) -> io::Result<Self> {
        cfg.endpoint.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "pb_tcp connector config is missing 'endpoint'",
            )
        })?;
        Ok(Self { cfg: cfg.clone() })
    }
}

#[async_trait::async_trait]
impl RunConnector for PbTcpRunConnector {
    async fn connect(&self, addr: String) -> io::Result<Box<dyn RunStream>> {
        let (host, port) = parse_address(addr.as_str())?;
        let pw = self.cfg.pw.as_ref().unwrap();

        let stream = TcpStream::connect(self.cfg.endpoint.as_ref().unwrap()).await?;
        let (reader, mut writer) = stream.into_split();

        write_conn_type(&mut writer, CONN_TYPE_STREAM).await?;

        let auth_req = StreamReq {
            auth: encrypt_field(pw, pw)?,
            dst_addr: Some(encrypt_field(&host, pw)?),
            dst_port: Some(port as u32),
            ..Default::default()
        };
        write_frame(&mut writer, &auth_req).await?;

        let mut stream = PbTcpClientRunStream::new(reader, writer, pw.clone(), port != 443);
        stream.set_info(&mut |x| {
            x.protocol_name = "pb_tcp".to_string();
            x.dst_port = Some(port);
            x.dst_addr = Some(host.to_string());
        });
        Ok(Box::new(stream))
    }

    async fn udp_tunnel(
        &self,
        _src_addr: String,
    ) -> io::Result<Option<(Box<dyn RunUdpReader>, Box<dyn RunUdpWriter>)>> {
        let pw = self.cfg.pw.as_ref().unwrap().clone();

        let stream = TcpStream::connect(self.cfg.endpoint.as_ref().unwrap()).await?;
        let (reader, mut writer) = stream.into_split();

        write_conn_type(&mut writer, CONN_TYPE_UDP).await?;

        let reader = Arc::new(Mutex::new(reader));
        let writer = Arc::new(Mutex::new(writer));

        Ok(Some((
            Box::new(PbTcpUdpClientReader::new(Arc::clone(&reader), pw.clone()))
                as Box<dyn RunUdpReader>,
            Box::new(PbTcpUdpClientWriter::new(
                Arc::clone(&writer),
                pw.clone(),
                pw,
            )) as Box<dyn RunUdpWriter>,
        )))
    }
}

use crate::def::{UDPMeta, UDPPacket};
use crate::proto::v1::pb::{UdpReq, UdpRes};

pub mod pb {
    tonic::include_proto!("moe.rikaaa0928.rog");
}



impl TryInto<UDPPacket> for UdpReq {
    type Error = ();

    fn try_into(self) -> Result<UDPPacket, Self::Error> {
        Ok(UDPPacket {
            meta: UDPMeta {
                dst_addr: self.dst_addr.unwrap(),
                dst_port: self.dst_port.unwrap() as u16,
                src_addr: self.src_addr.unwrap(),
                src_port: self.src_port.unwrap() as u16,
            },
            data: self.payload.unwrap(),
        })
    }
}

impl UdpReq {
    pub(crate) fn from_packet(packet: UDPPacket, auth: String) -> UdpReq {
        UdpReq {
            auth,
            payload: Some(packet.data),
            dst_addr: Some(packet.meta.dst_addr),
            dst_port: Some(packet.meta.dst_port as u32),
            src_addr: Some(packet.meta.src_addr),
            src_port: Some(packet.meta.src_port as u32),
        }
    }
}

impl TryInto<UDPPacket> for UdpRes {
    type Error = ();

    fn try_into(self) -> Result<UDPPacket, Self::Error> {
        Ok(UDPPacket {
            meta: UDPMeta {
                dst_addr: self.dst_addr.unwrap(),
                dst_port: self.dst_port.unwrap() as u16,
                src_addr: self.src_addr.unwrap(),
                src_port: self.src_port.unwrap() as u16,
            },
            data: self.payload,
        })
    }
}

impl TryFrom<UDPPacket> for UdpRes {
    type Error = ();

    fn try_from(value: UDPPacket) -> Result<Self, Self::Error> {
        Ok(Self {
            payload: value.data,
            dst_addr: Some(value.meta.dst_addr),
            dst_port: Some(value.meta.dst_port as u32),
            src_addr: Some(value.meta.src_addr),
            src_port: Some(value.meta.src_port as u32),
        })
    }
}
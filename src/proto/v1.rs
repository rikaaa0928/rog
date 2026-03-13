use crate::def::{UDPMeta, UDPPacket};
use crate::proto::v1::pb::{AddrInfo, RevUdpReq, RevUdpRes, UdpReq, UdpRes};

pub mod pb {
    tonic::include_proto!("moe.rikaaa0928.rog");
    tonic::include_proto!("moe.rikaaa0928.rev_rog");
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

// RevUdpReq (client -> server on the udp gRPC stream)
impl TryInto<UDPPacket> for RevUdpReq {
    type Error = ();

    fn try_into(self) -> Result<UDPPacket, Self::Error> {
        let info = self.addr_info.ok_or(())?;
        Ok(UDPPacket {
            meta: UDPMeta {
                dst_addr: info.dst_addr,
                dst_port: info.dst_port as u16,
                src_addr: info.src_addr,
                src_port: info.src_port as u16,
            },
            data: self.payload.unwrap_or_default(),
        })
    }
}

impl RevUdpReq {
    pub fn from_packet(packet: UDPPacket, auth: String) -> Self {
        RevUdpReq {
            auth,
            payload: Some(packet.data),
            addr_info: Some(AddrInfo {
                dst_addr: packet.meta.dst_addr,
                dst_port: packet.meta.dst_port as u32,
                src_addr: packet.meta.src_addr,
                src_port: packet.meta.src_port as u32,
            }),
            conn_id: None,
        }
    }
}

// RevUdpRes (server -> client on the udp gRPC stream)
impl TryInto<UDPPacket> for RevUdpRes {
    type Error = ();

    fn try_into(self) -> Result<UDPPacket, Self::Error> {
        let info = self.addr_info.ok_or(())?;
        Ok(UDPPacket {
            meta: UDPMeta {
                dst_addr: info.dst_addr,
                dst_port: info.dst_port as u16,
                src_addr: info.src_addr,
                src_port: info.src_port as u16,
            },
            data: self.payload,
        })
    }
}

impl TryFrom<UDPPacket> for RevUdpRes {
    type Error = ();

    fn try_from(value: UDPPacket) -> Result<Self, Self::Error> {
        Ok(Self {
            payload: value.data,
            addr_info: Some(AddrInfo {
                dst_addr: value.meta.dst_addr,
                dst_port: value.meta.dst_port as u32,
                src_addr: value.meta.src_addr,
                src_port: value.meta.src_port as u32,
            }),
        })
    }
}

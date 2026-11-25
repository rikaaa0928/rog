use ipnetwork::IpNetwork;
#[cfg(target_os = "linux")]
use proxy_observe::ConnectionMeta;
use proxy_observe::{ObserveConnection, ObserveRegistry};
use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;
#[cfg(target_os = "linux")]
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

#[allow(dead_code)]
pub(crate) const DIRECT_FLOW_EVENT_OPEN: u8 = 1;
#[allow(dead_code)]
pub(crate) const DIRECT_FLOW_EVENT_CLOSE: u8 = 2;
#[allow(dead_code)]
const BPF_RINGBUF_BUSY_BIT: u32 = 1 << 31;
#[allow(dead_code)]
const BPF_RINGBUF_DISCARD_BIT: u32 = 1 << 30;
#[allow(dead_code)]
const BPF_RINGBUF_HDR_SZ: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DirectFlowKey {
    pub family: u8,
    pub proto: u8,
    pub src_port_be: [u8; 2],
    pub dst_port_be: [u8; 2],
    pub _pad: [u8; 2],
    pub src_ip: [u8; 16],
    pub dst_ip: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirectFlowEvent {
    pub key: DirectFlowKey,
    pub kind: u8,
    pub tcp_flags: u8,
    pub _pad: [u8; 6],
}

impl DirectFlowKey {
    #[allow(dead_code)]
    pub(crate) fn from_tuple(
        udp: bool,
        src_ip: IpAddr,
        src_port: u16,
        dst_ip: IpAddr,
        dst_port: u16,
    ) -> Option<Self> {
        let src_ip = match src_ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
            other => other,
        };
        let dst_ip = match dst_ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
            other => other,
        };
        let proto = if udp { 17 } else { 6 };
        match (src_ip, dst_ip) {
            (IpAddr::V4(s4), IpAddr::V4(d4)) => {
                let mut src = [0u8; 16];
                let mut dst = [0u8; 16];
                src[..4].copy_from_slice(&s4.octets());
                dst[..4].copy_from_slice(&d4.octets());
                Some(Self {
                    family: 4,
                    proto,
                    src_port_be: src_port.to_be_bytes(),
                    dst_port_be: dst_port.to_be_bytes(),
                    _pad: [0; 2],
                    src_ip: src,
                    dst_ip: dst,
                })
            }
            (IpAddr::V6(s6), IpAddr::V6(d6)) => Some(Self {
                family: 6,
                proto,
                src_port_be: src_port.to_be_bytes(),
                dst_port_be: dst_port.to_be_bytes(),
                _pad: [0; 2],
                src_ip: s6.octets(),
                dst_ip: d6.octets(),
            }),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn to_tuple(&self) -> Option<(u8, IpAddr, u16, IpAddr, u16)> {
        let (src_ip, dst_ip) = match self.family {
            4 => (
                IpAddr::V4(Ipv4Addr::new(
                    self.src_ip[0],
                    self.src_ip[1],
                    self.src_ip[2],
                    self.src_ip[3],
                )),
                IpAddr::V4(Ipv4Addr::new(
                    self.dst_ip[0],
                    self.dst_ip[1],
                    self.dst_ip[2],
                    self.dst_ip[3],
                )),
            ),
            6 => (
                IpAddr::V6(Ipv6Addr::from(self.src_ip)),
                IpAddr::V6(Ipv6Addr::from(self.dst_ip)),
            ),
            _ => return None,
        };
        Some((
            self.proto,
            src_ip,
            u16::from_be_bytes(self.src_port_be),
            dst_ip,
            u16::from_be_bytes(self.dst_port_be),
        ))
    }

    #[allow(dead_code)]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                (self as *const Self) as *const u8,
                std::mem::size_of::<Self>(),
            )
        }
    }
}

#[allow(dead_code)]
pub(crate) fn decode_ringbuf_records(
    ring: &[u8],
    start_pos: u64,
    producer_pos: u64,
) -> (u64, Vec<DirectFlowEvent>) {
    let mut events = Vec::new();
    let ring_len = ring.len();
    if ring_len == 0 || !ring_len.is_power_of_two() {
        return (start_pos, events);
    }
    let mask = ring_len - 1;
    let mut cons_pos = start_pos;
    while cons_pos < producer_pos {
        let off = (cons_pos as usize) & mask;
        if off + BPF_RINGBUF_HDR_SZ > ring_len {
            break;
        }
        let len = u32::from_ne_bytes(ring[off..off + 4].try_into().unwrap());
        if (len & BPF_RINGBUF_BUSY_BIT) != 0 {
            break;
        }
        let data_len = (len & 0x3fff_ffff) as usize;
        let stride = (BPF_RINGBUF_HDR_SZ + ((data_len + 7) & !7)) as u64;
        if (len & BPF_RINGBUF_DISCARD_BIT) == 0 && data_len >= std::mem::size_of::<DirectFlowEvent>()
        {
            let data_off = off + BPF_RINGBUF_HDR_SZ;
            if data_off + std::mem::size_of::<DirectFlowEvent>() <= ring_len {
                let ev = unsafe {
                    std::ptr::read_unaligned(
                        ring.as_ptr().add(data_off) as *const DirectFlowEvent
                    )
                };
                events.push(ev);
            } else {
                let mut tmp = [0u8; std::mem::size_of::<DirectFlowEvent>()];
                for (i, b) in tmp.iter_mut().enumerate() {
                    *b = ring[(data_off + i) & mask];
                }
                let ev = unsafe {
                    std::ptr::read_unaligned(tmp.as_ptr() as *const DirectFlowEvent)
                };
                events.push(ev);
            }
        }
        cons_pos = cons_pos.saturating_add(stride);
    }
    (cons_pos, events)
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub(crate) enum BpfItem {
    Insn {
        code: u8,
        dst: u8,
        src: u8,
        off: i16,
        imm: i32,
    },
    Jmp {
        code: u8,
        dst: u8,
        src: u8,
        label: &'static str,
        imm: i32,
    },
    Label(&'static str),
}

#[allow(dead_code)]
pub(crate) fn assemble_bpf(items: &[BpfItem]) -> Vec<u8> {
    let mut labels = std::collections::HashMap::new();
    let mut pc: i32 = 0;
    for item in items {
        if let BpfItem::Label(name) = item {
            labels.insert(*name, pc);
        } else {
            pc += 1;
        }
    }

    let mut out = Vec::with_capacity((pc as usize) * 8);
    let mut cur_pc: i32 = 0;
    for item in items {
        let (code, dst, src, off, imm) = match *item {
            BpfItem::Label(_) => continue,
            BpfItem::Insn {
                code,
                dst,
                src,
                off,
                imm,
            } => (code, dst, src, off, imm),
            BpfItem::Jmp {
                code,
                dst,
                src,
                label,
                imm,
            } => {
                let target_pc = *labels.get(label).expect("missing BPF label");
                let rel = (target_pc - cur_pc - 1) as i16;
                (code, dst, src, rel, imm)
            }
        };
        out.push(code);
        out.push((src << 4) | (dst & 0x0f));
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&imm.to_le_bytes());
        cur_pc += 1;
    }
    out
}

#[allow(dead_code)]
pub(crate) fn build_tc_direct_prog_bytecode(
    v4_lru_fd: i32,
    v4_lpm_fd: i32,
    v6_lru_fd: i32,
    v6_lpm_fd: i32,
) -> Vec<u8> {
    build_tc_direct_prog_bytecode_inner(v4_lru_fd, v4_lpm_fd, v6_lru_fd, v6_lpm_fd, None)
}

#[allow(dead_code)]
pub(crate) fn build_tc_direct_prog_with_flow_log_bytecode(
    v4_lru_fd: i32,
    v4_lpm_fd: i32,
    v6_lru_fd: i32,
    v6_lpm_fd: i32,
    flow_seen_fd: i32,
    ringbuf_fd: i32,
) -> Vec<u8> {
    build_tc_direct_prog_bytecode_inner(
        v4_lru_fd,
        v4_lpm_fd,
        v6_lru_fd,
        v6_lpm_fd,
        Some((flow_seen_fd, ringbuf_fd)),
    )
}

#[allow(dead_code)]
fn build_tc_direct_prog_bytecode_inner(
    v4_lru_fd: i32,
    v4_lpm_fd: i32,
    v6_lru_fd: i32,
    v6_lpm_fd: i32,
    flow_log: Option<(i32, i32)>,
) -> Vec<u8> {
    use BpfItem::*;
    let map_ld = |dst: u8, fd: i32| -> [BpfItem; 2] {
        [
            Insn {
                code: 0x18,
                dst,
                src: 1,
                off: 0,
                imm: fd,
            },
            Insn {
                code: 0x00,
                dst: 0,
                src: 0,
                off: 0,
                imm: 0,
            },
        ]
    };
    let imm64_ld = |dst: u8, val: u64| -> [BpfItem; 2] {
        [
            Insn {
                code: 0x18,
                dst,
                src: 0,
                off: 0,
                imm: (val as u32) as i32,
            },
            Insn {
                code: 0x00,
                dst: 0,
                src: 0,
                off: 0,
                imm: ((val >> 32) as u32) as i32,
            },
        ]
    };

    let mut items = Vec::with_capacity(if flow_log.is_some() { 220 } else { 64 });
    // r6 = r1 (skb)
    items.push(Insn {
        code: 0xbf,
        dst: 6,
        src: 1,
        off: 0,
        imm: 0,
    });
    // r2 = *(u32 *)(r6 + 76) (skb->data)
    items.push(Insn {
        code: 0x61,
        dst: 2,
        src: 6,
        off: 76,
        imm: 0,
    });
    // r3 = *(u32 *)(r6 + 80) (skb->data_end)
    items.push(Insn {
        code: 0x61,
        dst: 3,
        src: 6,
        off: 80,
        imm: 0,
    });
    // r4 = r2 + 14
    items.push(Insn {
        code: 0xbf,
        dst: 4,
        src: 2,
        off: 0,
        imm: 0,
    });
    items.push(Insn {
        code: 0x07,
        dst: 4,
        src: 0,
        off: 0,
        imm: 14,
    });
    // if r4 > r3 goto exit
    items.push(Jmp {
        code: 0x2d,
        dst: 4,
        src: 3,
        label: "exit",
        imm: 0,
    });
    // r5 = *(u16 *)(r2 + 12) (eth_proto)
    items.push(Insn {
        code: 0x69,
        dst: 5,
        src: 2,
        off: 12,
        imm: 0,
    });
    // if r5 != htons(ETH_P_IP) (0x0008 on LE) goto check_v6
    items.push(Jmp {
        code: 0x55,
        dst: 5,
        src: 0,
        label: "check_v6",
        imm: 0x0008,
    });

    // IPv4 branch:
    items.push(Insn {
        code: 0xbf,
        dst: 4,
        src: 2,
        off: 0,
        imm: 0,
    });
    items.push(Insn {
        code: 0x07,
        dst: 4,
        src: 0,
        off: 0,
        imm: 34,
    });
    items.push(Jmp {
        code: 0x2d,
        dst: 4,
        src: 3,
        label: "exit",
        imm: 0,
    });
    if flow_log.is_some() {
        items.push(Insn {
            code: 0xb7,
            dst: 8,
            src: 0,
            off: 0,
            imm: 4,
        }); // r8 = family 4
    }
    // r7 = *(u32 *)(r2 + 30) (ip4 daddr)
    items.push(Insn {
        code: 0x61,
        dst: 7,
        src: 2,
        off: 30,
        imm: 0,
    });
    // *(u32 *)(r10 - 8) = 32 (prefixlen)
    items.push(Insn {
        code: 0xb7,
        dst: 0,
        src: 0,
        off: 0,
        imm: 32,
    });
    items.push(Insn {
        code: 0x63,
        dst: 10,
        src: 0,
        off: -8,
        imm: 0,
    });
    // *(u32 *)(r10 - 4) = daddr
    items.push(Insn {
        code: 0x63,
        dst: 10,
        src: 7,
        off: -4,
        imm: 0,
    });
    // lookup in v4_lru
    items.extend_from_slice(&map_ld(1, v4_lru_fd));
    items.push(Insn {
        code: 0xbf,
        dst: 2,
        src: 10,
        off: 0,
        imm: 0,
    });
    items.push(Insn {
        code: 0x07,
        dst: 2,
        src: 0,
        off: 0,
        imm: -4,
    });
    items.push(Insn {
        code: 0x85,
        dst: 0,
        src: 0,
        off: 0,
        imm: 1,
    }); // bpf_map_lookup_elem
    items.push(Jmp {
        code: 0x15,
        dst: 0,
        src: 0,
        label: "lpm_v4",
        imm: 0,
    });
    items.push(Insn {
        code: 0x79,
        dst: 7,
        src: 0,
        off: 0,
        imm: 0,
    }); // r7 = expire_ns
    items.push(Insn {
        code: 0x85,
        dst: 0,
        src: 0,
        off: 0,
        imm: 5,
    }); // bpf_ktime_get_ns
    items.push(Jmp {
        code: 0x2d,
        dst: 7,
        src: 0,
        label: "mark_direct",
        imm: 0,
    });

    items.push(Label("lpm_v4"));
    items.extend_from_slice(&map_ld(1, v4_lpm_fd));
    items.push(Insn {
        code: 0xbf,
        dst: 2,
        src: 10,
        off: 0,
        imm: 0,
    });
    items.push(Insn {
        code: 0x07,
        dst: 2,
        src: 0,
        off: 0,
        imm: -8,
    });
    items.push(Jmp {
        code: 0x05,
        dst: 0,
        src: 0,
        label: "do_lpm",
        imm: 0,
    });

    // IPv6 branch:
    items.push(Label("check_v6"));
    items.push(Jmp {
        code: 0x55,
        dst: 5,
        src: 0,
        label: "exit",
        imm: 0xdd86,
    });
    items.push(Insn {
        code: 0xbf,
        dst: 4,
        src: 2,
        off: 0,
        imm: 0,
    });
    items.push(Insn {
        code: 0x07,
        dst: 4,
        src: 0,
        off: 0,
        imm: 54,
    });
    items.push(Jmp {
        code: 0x2d,
        dst: 4,
        src: 3,
        label: "exit",
        imm: 0,
    });
    if flow_log.is_some() {
        items.push(Insn {
            code: 0xb7,
            dst: 8,
            src: 0,
            off: 0,
            imm: 6,
        }); // r8 = family 6
    }
    // *(u32 *)(r10 - 24) = 128 (prefixlen)
    items.push(Insn {
        code: 0xb7,
        dst: 0,
        src: 0,
        off: 0,
        imm: 128,
    });
    items.push(Insn {
        code: 0x63,
        dst: 10,
        src: 0,
        off: -24,
        imm: 0,
    });
    // copy 16B IPv6 daddr from r2 + 38..54 to r10 - 20..-4
    for (pkt_off, stk_off) in [(38i16, -20i16), (42, -16), (46, -12), (50, -8)] {
        items.push(Insn {
            code: 0x61,
            dst: 0,
            src: 2,
            off: pkt_off,
            imm: 0,
        });
        items.push(Insn {
            code: 0x63,
            dst: 10,
            src: 0,
            off: stk_off,
            imm: 0,
        });
    }
    // lookup in v6_lru
    items.extend_from_slice(&map_ld(1, v6_lru_fd));
    items.push(Insn {
        code: 0xbf,
        dst: 2,
        src: 10,
        off: 0,
        imm: 0,
    });
    items.push(Insn {
        code: 0x07,
        dst: 2,
        src: 0,
        off: 0,
        imm: -20,
    });
    items.push(Insn {
        code: 0x85,
        dst: 0,
        src: 0,
        off: 0,
        imm: 1,
    }); // bpf_map_lookup_elem
    items.push(Jmp {
        code: 0x15,
        dst: 0,
        src: 0,
        label: "lpm_v6",
        imm: 0,
    });
    items.push(Insn {
        code: 0x79,
        dst: 7,
        src: 0,
        off: 0,
        imm: 0,
    }); // r7 = expire_ns
    items.push(Insn {
        code: 0x85,
        dst: 0,
        src: 0,
        off: 0,
        imm: 5,
    }); // bpf_ktime_get_ns
    items.push(Jmp {
        code: 0x2d,
        dst: 7,
        src: 0,
        label: "mark_direct",
        imm: 0,
    });

    items.push(Label("lpm_v6"));
    items.extend_from_slice(&map_ld(1, v6_lpm_fd));
    items.push(Insn {
        code: 0xbf,
        dst: 2,
        src: 10,
        off: 0,
        imm: 0,
    });
    items.push(Insn {
        code: 0x07,
        dst: 2,
        src: 0,
        off: 0,
        imm: -24,
    });

    items.push(Label("do_lpm"));
    items.push(Insn {
        code: 0x85,
        dst: 0,
        src: 0,
        off: 0,
        imm: 1,
    }); // bpf_map_lookup_elem
    items.push(Jmp {
        code: 0x15,
        dst: 0,
        src: 0,
        label: "exit",
        imm: 0,
    });

    items.push(Label("mark_direct"));
    items.push(Insn {
        code: 0x61,
        dst: 0,
        src: 6,
        off: 8,
        imm: 0,
    }); // r0 = skb->mark
    items.push(Insn {
        code: 0x57,
        dst: 0,
        src: 0,
        off: 0,
        imm: -256,
    }); // r0 &= ~0xff
    items.push(Insn {
        code: 0x47,
        dst: 0,
        src: 0,
        off: 0,
        imm: 0x78,
    }); // r0 |= 0x78
    items.push(Insn {
        code: 0x63,
        dst: 6,
        src: 0,
        off: 8,
        imm: 0,
    }); // skb->mark = r0

    if let Some((flow_seen_fd, ringbuf_fd)) = flow_log {
        // Zero DirectFlowEvent (48 bytes at r10 - 48 .. r10 - 0)
        items.push(Insn {
            code: 0xb7,
            dst: 0,
            src: 0,
            off: 0,
            imm: 0,
        });
        for off in [-48i16, -40, -32, -24, -16, -8] {
            items.push(Insn {
                code: 0x7b,
                dst: 10,
                src: 0,
                off,
                imm: 0,
            });
        }
        // event.key.family = r8 (4 or 6)
        items.push(Insn {
            code: 0x73,
            dst: 10,
            src: 8,
            off: -48,
            imm: 0,
        });
        items.push(Jmp {
            code: 0x55,
            dst: 8,
            src: 0,
            label: "flow_v6",
            imm: 4,
        });

        // flow_v4: bpf_skb_load_bytes(skb=r6, offset=14, to=r10-88, len=20)
        items.push(Insn {
            code: 0xbf,
            dst: 1,
            src: 6,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 2,
            src: 0,
            off: 0,
            imm: 14,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 3,
            src: 10,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 3,
            src: 0,
            off: 0,
            imm: -88,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 4,
            src: 0,
            off: 0,
            imm: 20,
        });
        items.push(Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 26,
        }); // bpf_skb_load_bytes
        items.push(Jmp {
            code: 0x55,
            dst: 0,
            src: 0,
            label: "exit",
            imm: 0,
        });

        // Skip non-first IPv4 fragments (frag_off & 0x1fff != 0 in network order)
        items.push(Insn {
            code: 0x69,
            dst: 1,
            src: 10,
            off: -82,
            imm: 0,
        });
        items.push(Insn {
            code: 0x57,
            dst: 1,
            src: 0,
            off: 0,
            imm: u16::from_le_bytes(0x1fffu16.to_be_bytes()) as i32,
        });
        items.push(Jmp {
            code: 0x55,
            dst: 1,
            src: 0,
            label: "exit",
            imm: 0,
        });

        // r1 = ihl_bytes = ((*(u8 *)(r10 - 88)) & 0x0f) << 2
        items.push(Insn {
            code: 0x71,
            dst: 1,
            src: 10,
            off: -88,
            imm: 0,
        });
        items.push(Insn {
            code: 0x57,
            dst: 1,
            src: 0,
            off: 0,
            imm: 0x0f,
        });
        items.push(Insn {
            code: 0x67,
            dst: 1,
            src: 0,
            off: 0,
            imm: 2,
        });
        items.push(Jmp {
            code: 0xa5,
            dst: 1,
            src: 0,
            label: "exit",
            imm: 20,
        }); // if ihl_bytes < 20 goto exit
        items.push(Insn {
            code: 0xbf,
            dst: 9,
            src: 1,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 9,
            src: 0,
            off: 0,
            imm: 14,
        }); // r9 = l4_off (34..=74)
        // r7 = protocol = *(u8 *)(r10 - 79)
        items.push(Insn {
            code: 0x71,
            dst: 7,
            src: 10,
            off: -79,
            imm: 0,
        });
        items.push(Insn {
            code: 0x73,
            dst: 10,
            src: 7,
            off: -47,
            imm: 0,
        }); // key.proto = r7
        // copy saddr (r10 - 76 -> r10 - 40) and daddr (r10 - 72 -> r10 - 24)
        items.push(Insn {
            code: 0x61,
            dst: 1,
            src: 10,
            off: -76,
            imm: 0,
        });
        items.push(Insn {
            code: 0x63,
            dst: 10,
            src: 1,
            off: -40,
            imm: 0,
        });
        items.push(Insn {
            code: 0x61,
            dst: 1,
            src: 10,
            off: -72,
            imm: 0,
        });
        items.push(Insn {
            code: 0x63,
            dst: 10,
            src: 1,
            off: -24,
            imm: 0,
        });
        items.push(Jmp {
            code: 0x05,
            dst: 0,
            src: 0,
            label: "flow_l4",
            imm: 0,
        });

        // flow_v6: bpf_skb_load_bytes(skb=r6, offset=14, to=r10-88, len=40)
        items.push(Label("flow_v6"));
        items.push(Insn {
            code: 0xbf,
            dst: 1,
            src: 6,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 2,
            src: 0,
            off: 0,
            imm: 14,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 3,
            src: 10,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 3,
            src: 0,
            off: 0,
            imm: -88,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 4,
            src: 0,
            off: 0,
            imm: 40,
        });
        items.push(Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 26,
        });
        items.push(Jmp {
            code: 0x55,
            dst: 0,
            src: 0,
            label: "exit",
            imm: 0,
        });
        // r7 = nexthdr = *(u8 *)(r10 - 82)
        items.push(Insn {
            code: 0x71,
            dst: 7,
            src: 10,
            off: -82,
            imm: 0,
        });
        items.push(Insn {
            code: 0x73,
            dst: 10,
            src: 7,
            off: -47,
            imm: 0,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 9,
            src: 0,
            off: 0,
            imm: 54,
        }); // r9 = l4_off = 14 + 40
        // copy IPv6 saddr (r10 - 80..-64 -> r10 - 40..-24) and daddr (r10 - 64..-48 -> r10 - 24..-8)
        for (src_off, dst_off) in [(-80i16, -40i16), (-72, -32), (-64, -24), (-56, -16)] {
            items.push(Insn {
                code: 0x79,
                dst: 1,
                src: 10,
                off: src_off,
                imm: 0,
            });
            items.push(Insn {
                code: 0x7b,
                dst: 10,
                src: 1,
                off: dst_off,
                imm: 0,
            });
        }

        // flow_l4: r7 = proto, r9 = l4_off
        items.push(Label("flow_l4"));
        items.push(Jmp {
            code: 0x15,
            dst: 7,
            src: 0,
            label: "flow_tcp",
            imm: 6,
        });
        items.push(Jmp {
            code: 0x15,
            dst: 7,
            src: 0,
            label: "flow_udp",
            imm: 17,
        });
        items.push(Jmp {
            code: 0x05,
            dst: 0,
            src: 0,
            label: "exit",
            imm: 0,
        });

        // flow_tcp: load 14 bytes at r9 into r10 - 88
        items.push(Label("flow_tcp"));
        items.push(Insn {
            code: 0xbf,
            dst: 1,
            src: 6,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 9,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 3,
            src: 10,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 3,
            src: 0,
            off: 0,
            imm: -88,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 4,
            src: 0,
            off: 0,
            imm: 14,
        });
        items.push(Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 26,
        });
        items.push(Jmp {
            code: 0x55,
            dst: 0,
            src: 0,
            label: "exit",
            imm: 0,
        });
        // r8 = tcp_flags = *(u8 *)(r10 - 75); event.tcp_flags = r8
        items.push(Insn {
            code: 0x71,
            dst: 8,
            src: 10,
            off: -75,
            imm: 0,
        });
        items.push(Insn {
            code: 0x73,
            dst: 10,
            src: 8,
            off: -7,
            imm: 0,
        });
        items.push(Jmp {
            code: 0x05,
            dst: 0,
            src: 0,
            label: "flow_ports",
            imm: 0,
        });

        // flow_udp: load 4 bytes at r9 into r10 - 88
        items.push(Label("flow_udp"));
        items.push(Insn {
            code: 0xbf,
            dst: 1,
            src: 6,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 9,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 3,
            src: 10,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 3,
            src: 0,
            off: 0,
            imm: -88,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 4,
            src: 0,
            off: 0,
            imm: 4,
        });
        items.push(Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 26,
        });
        items.push(Jmp {
            code: 0x55,
            dst: 0,
            src: 0,
            label: "exit",
            imm: 0,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 8,
            src: 0,
            off: 0,
            imm: 0,
        }); // r8 = 0 (tcp_flags)

        // flow_ports: copy src_port (r10 - 88 -> r10 - 46) and dst_port (r10 - 86 -> r10 - 44)
        items.push(Label("flow_ports"));
        items.push(Insn {
            code: 0x69,
            dst: 1,
            src: 10,
            off: -88,
            imm: 0,
        });
        items.push(Insn {
            code: 0x6b,
            dst: 10,
            src: 1,
            off: -46,
            imm: 0,
        });
        items.push(Insn {
            code: 0x69,
            dst: 1,
            src: 10,
            off: -86,
            imm: 0,
        });
        items.push(Insn {
            code: 0x6b,
            dst: 10,
            src: 1,
            off: -44,
            imm: 0,
        });

        // r9 = bpf_ktime_get_ns()
        items.push(Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 5,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 9,
            src: 0,
            off: 0,
            imm: 0,
        });

        // Lookup flow_seen(key = r10 - 48)
        items.extend_from_slice(&map_ld(1, flow_seen_fd));
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 10,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 2,
            src: 0,
            off: 0,
            imm: -48,
        });
        items.push(Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 1,
        }); // bpf_map_lookup_elem
        items.push(Jmp {
            code: 0x15,
            dst: 0,
            src: 0,
            label: "emit_open",
            imm: 0,
        });

        // Hit in flow_seen: r7 = map_value_ptr
        items.push(Insn {
            code: 0xbf,
            dst: 7,
            src: 0,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x79,
            dst: 1,
            src: 7,
            off: 0,
            imm: 0,
        }); // r1 = state_ns
        items.push(Jmp {
            code: 0xc5,
            dst: 1,
            src: 0,
            label: "hit_closed",
            imm: 0,
        }); // if (s64)r1 < 0 goto hit_closed

        // hit_open: if (r8 & 0x05) != 0 goto emit_close
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 8,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x57,
            dst: 2,
            src: 0,
            off: 0,
            imm: 0x05,
        });
        items.push(Jmp {
            code: 0x55,
            dst: 2,
            src: 0,
            label: "emit_close",
            imm: 0,
        });
        // Slide last_seen_ns = r9 in-place
        items.push(Insn {
            code: 0x7b,
            dst: 7,
            src: 9,
            off: 0,
            imm: 0,
        });
        items.push(Jmp {
            code: 0x05,
            dst: 0,
            src: 0,
            label: "exit",
            imm: 0,
        });

        // hit_closed: allow fast re-open only on pure SYN ((r8 & 0x17) == 0x02),
        // ignore FIN/RST, otherwise enforce 10s TIME_WAIT guard.
        items.push(Label("hit_closed"));
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 8,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x57,
            dst: 2,
            src: 0,
            off: 0,
            imm: 0x17,
        });
        items.push(Jmp {
            code: 0x15,
            dst: 2,
            src: 0,
            label: "emit_open",
            imm: 0x02,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 8,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x57,
            dst: 2,
            src: 0,
            off: 0,
            imm: 0x05,
        });
        items.push(Jmp {
            code: 0x55,
            dst: 2,
            src: 0,
            label: "exit",
            imm: 0,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 1,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x67,
            dst: 2,
            src: 0,
            off: 0,
            imm: 1,
        });
        items.push(Insn {
            code: 0x77,
            dst: 2,
            src: 0,
            off: 0,
            imm: 1,
        }); // r2 = closed_ns (bit 63 cleared)
        items.push(Jmp {
            code: 0xad,
            dst: 9,
            src: 2,
            label: "exit",
            imm: 0,
        }); // if now_ns < closed_ns goto exit
        items.push(Insn {
            code: 0xbf,
            dst: 3,
            src: 9,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x1f,
            dst: 3,
            src: 2,
            off: 0,
            imm: 0,
        }); // r3 = now_ns - closed_ns
        items.extend_from_slice(&imm64_ld(4, 10_000_000_000));
        items.push(Jmp {
            code: 0xad,
            dst: 3,
            src: 4,
            label: "exit",
            imm: 0,
        }); // if elapsed < 10s goto exit

        // emit_open: ignore stray FIN/RST packets
        items.push(Label("emit_open"));
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 8,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x57,
            dst: 2,
            src: 0,
            off: 0,
            imm: 0x05,
        });
        items.push(Jmp {
            code: 0x55,
            dst: 2,
            src: 0,
            label: "exit",
            imm: 0,
        });
        items.push(Insn {
            code: 0x7b,
            dst: 10,
            src: 9,
            off: -96,
            imm: 0,
        }); // *(u64 *)(r10 - 96) = now_ns
        items.extend_from_slice(&map_ld(1, flow_seen_fd));
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 10,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 2,
            src: 0,
            off: 0,
            imm: -48,
        });
        items.push(Insn {
            code: 0xbf,
            dst: 3,
            src: 10,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 3,
            src: 0,
            off: 0,
            imm: -96,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 4,
            src: 0,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 2,
        }); // bpf_map_update_elem
        items.push(Insn {
            code: 0xb7,
            dst: 1,
            src: 0,
            off: 0,
            imm: DIRECT_FLOW_EVENT_OPEN as i32,
        });
        items.push(Insn {
            code: 0x73,
            dst: 10,
            src: 1,
            off: -8,
            imm: 0,
        }); // event.kind = OPEN
        items.extend_from_slice(&map_ld(1, ringbuf_fd));
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 10,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 2,
            src: 0,
            off: 0,
            imm: -48,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 3,
            src: 0,
            off: 0,
            imm: std::mem::size_of::<DirectFlowEvent>() as i32,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 4,
            src: 0,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 130,
        }); // bpf_ringbuf_output
        items.push(Jmp {
            code: 0x05,
            dst: 0,
            src: 0,
            label: "exit",
            imm: 0,
        });

        // emit_close: set CLOSED bit (1 << 63) | now_ns in flow_seen and emit CLOSE event
        items.push(Label("emit_close"));
        items.push(Insn {
            code: 0xb7,
            dst: 2,
            src: 0,
            off: 0,
            imm: 1,
        });
        items.push(Insn {
            code: 0x67,
            dst: 2,
            src: 0,
            off: 0,
            imm: 63,
        });
        items.push(Insn {
            code: 0x4f,
            dst: 2,
            src: 9,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x7b,
            dst: 7,
            src: 2,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 1,
            src: 0,
            off: 0,
            imm: DIRECT_FLOW_EVENT_CLOSE as i32,
        });
        items.push(Insn {
            code: 0x73,
            dst: 10,
            src: 1,
            off: -8,
            imm: 0,
        }); // event.kind = CLOSE
        items.extend_from_slice(&map_ld(1, ringbuf_fd));
        items.push(Insn {
            code: 0xbf,
            dst: 2,
            src: 10,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x07,
            dst: 2,
            src: 0,
            off: 0,
            imm: -48,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 3,
            src: 0,
            off: 0,
            imm: std::mem::size_of::<DirectFlowEvent>() as i32,
        });
        items.push(Insn {
            code: 0xb7,
            dst: 4,
            src: 0,
            off: 0,
            imm: 0,
        });
        items.push(Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 130,
        }); // bpf_ringbuf_output
    }

    items.push(Label("exit"));
    items.push(Insn {
        code: 0xb7,
        dst: 0,
        src: 0,
        off: 0,
        imm: 0,
    }); // TC_ACT_OK
    items.push(Insn {
        code: 0x95,
        dst: 0,
        src: 0,
        off: 0,
        imm: 0,
    }); // exit

    assemble_bpf(&items)
}

#[allow(dead_code)]
pub(crate) fn build_sockhash_verdict_bytecode(sockhash_fd: i32) -> Vec<u8> {
    use BpfItem::*;
    let items = [
        Insn {
            code: 0xbf,
            dst: 6,
            src: 1,
            off: 0,
            imm: 0,
        }, // r6 = r1 (skb)
        Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 46,
        }, // call bpf_get_socket_cookie
        Insn {
            code: 0x7b,
            dst: 10,
            src: 0,
            off: -8,
            imm: 0,
        }, // *(u64 *)(r10 - 8) = r0
        Insn {
            code: 0xbf,
            dst: 1,
            src: 6,
            off: 0,
            imm: 0,
        }, // r1 = r6
        Insn {
            code: 0x18,
            dst: 2,
            src: 1,
            off: 0,
            imm: sockhash_fd,
        }, // ld_imm64 r2, map_fd
        Insn {
            code: 0x00,
            dst: 0,
            src: 0,
            off: 0,
            imm: 0,
        },
        Insn {
            code: 0xbf,
            dst: 3,
            src: 10,
            off: 0,
            imm: 0,
        }, // r3 = r10
        Insn {
            code: 0x07,
            dst: 3,
            src: 0,
            off: 0,
            imm: -8,
        }, // r3 += -8
        Insn {
            code: 0xb7,
            dst: 4,
            src: 0,
            off: 0,
            imm: 0,
        }, // r4 = 0 (flags)
        Insn {
            code: 0x85,
            dst: 0,
            src: 0,
            off: 0,
            imm: 72,
        }, // call bpf_sk_redirect_hash
        Insn {
            code: 0x95,
            dst: 0,
            src: 0,
            off: 0,
            imm: 0,
        }, // exit
    ];
    assemble_bpf(&items)
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::*;
    use log::{debug, info, warn};
    use std::collections::HashMap;
    use std::mem;
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;
    use tokio::io::unix::AsyncFd;
    use tokio::sync::Notify;

    const BPF_MAP_CREATE: libc::c_int = 0;
    const BPF_MAP_LOOKUP_ELEM: libc::c_int = 1;
    const BPF_MAP_UPDATE_ELEM: libc::c_int = 2;
    const BPF_MAP_DELETE_ELEM: libc::c_int = 3;
    const BPF_PROG_LOAD: libc::c_int = 5;
    const BPF_PROG_ATTACH: libc::c_int = 8;

    const BPF_MAP_TYPE_LRU_HASH: u32 = 9;
    const BPF_MAP_TYPE_LPM_TRIE: u32 = 11;
    const BPF_MAP_TYPE_SOCKHASH: u32 = 18;
    const BPF_MAP_TYPE_RINGBUF: u32 = 27;

    const BPF_PROG_TYPE_SCHED_CLS: u32 = 3;
    const BPF_PROG_TYPE_SK_SKB: u32 = 14;
    const BPF_SK_SKB_STREAM_VERDICT: u32 = 5;
    const BPF_F_NO_PREALLOC: u32 = 1;
    const SO_COOKIE: libc::c_int = 57;

    struct DirectBpfState {
        v4_lru: OwnedFd,
        v4_lpm: OwnedFd,
        v6_lru: OwnedFd,
        v6_lpm: OwnedFd,
        flow_seen: Option<OwnedFd>,
        _prog: OwnedFd,
        active_cidrs: Mutex<HashSet<IpNetwork>>,
    }

    struct SockHashState {
        map_fd: OwnedFd,
        _prog_fd: OwnedFd,
    }

    static DIRECT_ENABLED: AtomicBool = AtomicBool::new(false);
    static DIRECT_DISABLED: AtomicBool = AtomicBool::new(false);
    static DIRECT_STATE: OnceLock<DirectBpfState> = OnceLock::new();
    static CIDR_REFRESH_NOTIFY: OnceLock<Arc<Notify>> = OnceLock::new();
    static SOCKHASH_STATE: OnceLock<Option<SockHashState>> = OnceLock::new();

    fn bpf_syscall(cmd: libc::c_int, attr: &[u8]) -> io::Result<i32> {
        let ret = unsafe {
            libc::syscall(
                libc::SYS_bpf,
                cmd,
                attr.as_ptr() as *const libc::c_void,
                attr.len() as u32,
            )
        };
        if ret < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(ret as i32)
        }
    }

    fn create_bpf_map(
        map_type: u32,
        key_size: u32,
        value_size: u32,
        max_entries: u32,
        map_flags: u32,
    ) -> io::Result<OwnedFd> {
        let mut attr = [0u8; 128];
        attr[0..4].copy_from_slice(&map_type.to_ne_bytes());
        attr[4..8].copy_from_slice(&key_size.to_ne_bytes());
        attr[8..12].copy_from_slice(&value_size.to_ne_bytes());
        attr[12..16].copy_from_slice(&max_entries.to_ne_bytes());
        attr[16..20].copy_from_slice(&map_flags.to_ne_bytes());
        let fd = bpf_syscall(BPF_MAP_CREATE, &attr)?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn load_bpf_prog(prog_type: u32, bytecode: &[u8]) -> io::Result<OwnedFd> {
        let license = b"GPL\0";
        let insn_cnt = (bytecode.len() / 8) as u32;
        let insns_ptr = bytecode.as_ptr() as usize as u64;
        let lic_ptr = license.as_ptr() as usize as u64;

        let mut attr = [0u8; 128];
        attr[0..4].copy_from_slice(&prog_type.to_ne_bytes());
        attr[4..8].copy_from_slice(&insn_cnt.to_ne_bytes());
        attr[8..16].copy_from_slice(&insns_ptr.to_ne_bytes());
        attr[16..24].copy_from_slice(&lic_ptr.to_ne_bytes());

        let fd = bpf_syscall(BPF_PROG_LOAD, &attr)?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn attach_bpf_prog(target_fd: i32, attach_bpf_fd: i32, attach_type: u32) -> io::Result<()> {
        let mut attr = [0u8; 128];
        attr[0..4].copy_from_slice(&(target_fd as u32).to_ne_bytes());
        attr[4..8].copy_from_slice(&(attach_bpf_fd as u32).to_ne_bytes());
        attr[8..12].copy_from_slice(&attach_type.to_ne_bytes());
        bpf_syscall(BPF_PROG_ATTACH, &attr)?;
        Ok(())
    }

    fn map_lookup_elem(map_fd: i32, key: &[u8], value: &mut [u8]) -> io::Result<()> {
        let mut attr = [0u8; 128];
        let key_ptr = key.as_ptr() as usize as u64;
        let val_ptr = value.as_mut_ptr() as usize as u64;
        attr[0..4].copy_from_slice(&(map_fd as u32).to_ne_bytes());
        attr[8..16].copy_from_slice(&key_ptr.to_ne_bytes());
        attr[16..24].copy_from_slice(&val_ptr.to_ne_bytes());
        bpf_syscall(BPF_MAP_LOOKUP_ELEM, &attr)?;
        Ok(())
    }

    fn map_update_elem(map_fd: i32, key: &[u8], value: &[u8], flags: u64) -> io::Result<()> {
        let mut attr = [0u8; 128];
        let key_ptr = key.as_ptr() as usize as u64;
        let val_ptr = value.as_ptr() as usize as u64;
        attr[0..4].copy_from_slice(&(map_fd as u32).to_ne_bytes());
        attr[8..16].copy_from_slice(&key_ptr.to_ne_bytes());
        attr[16..24].copy_from_slice(&val_ptr.to_ne_bytes());
        attr[24..32].copy_from_slice(&flags.to_ne_bytes());
        bpf_syscall(BPF_MAP_UPDATE_ELEM, &attr)?;
        Ok(())
    }

    fn map_delete_elem(map_fd: i32, key: &[u8]) -> io::Result<()> {
        let mut attr = [0u8; 128];
        let key_ptr = key.as_ptr() as usize as u64;
        attr[0..4].copy_from_slice(&(map_fd as u32).to_ne_bytes());
        attr[8..16].copy_from_slice(&key_ptr.to_ne_bytes());
        bpf_syscall(BPF_MAP_DELETE_ELEM, &attr)?;
        Ok(())
    }

    fn monotonic_ns() -> u64 {
        unsafe {
            let mut ts: libc::timespec = mem::zeroed();
            if libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) == 0 {
                (ts.tv_sec as u64)
                    .saturating_mul(1_000_000_000)
                    .saturating_add(ts.tv_nsec as u64)
            } else {
                0
            }
        }
    }

    fn detect_default_ifname() -> Option<(String, i32)> {
        let content = std::fs::read_to_string("/proc/net/route").ok()?;
        for line in content.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 && parts[1] == "00000000" {
                let ifname = parts[0].to_string();
                let c_if = std::ffi::CString::new(ifname.as_str()).ok()?;
                let idx = unsafe { libc::if_nametoindex(c_if.as_ptr()) };
                if idx > 0 {
                    return Some((ifname, idx as i32));
                }
            }
        }
        None
    }

    fn nla(atype: u16, data: &[u8]) -> Vec<u8> {
        let elen = (4 + data.len()) as u16;
        let pad = (4 - (elen as usize % 4)) % 4;
        let mut v = Vec::with_capacity(elen as usize + pad);
        v.extend_from_slice(&elen.to_ne_bytes());
        v.extend_from_slice(&atype.to_ne_bytes());
        v.extend_from_slice(data);
        v.resize(elen as usize + pad, 0);
        v
    }

    fn send_netlink_msg(
        nl_fd: i32,
        msg_type: u16,
        flags: u16,
        seq: u32,
        payload: &[u8],
    ) -> io::Result<()> {
        let total_len = (16 + payload.len()) as u32;
        let mut msg = Vec::with_capacity(total_len as usize);
        msg.extend_from_slice(&total_len.to_ne_bytes());
        msg.extend_from_slice(&msg_type.to_ne_bytes());
        msg.extend_from_slice(&flags.to_ne_bytes());
        msg.extend_from_slice(&seq.to_ne_bytes());
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(payload);

        let sent = unsafe { libc::send(nl_fd, msg.as_ptr() as *const libc::c_void, msg.len(), 0) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut resp = [0u8; 1024];
        let n = unsafe { libc::recv(nl_fd, resp.as_mut_ptr() as *mut libc::c_void, resp.len(), 0) };
        if n < 20 {
            return Err(io::Error::last_os_error());
        }
        let nlmsg_type = u16::from_ne_bytes([resp[4], resp[5]]);
        if nlmsg_type == 2 {
            let err_code = i32::from_ne_bytes([resp[16], resp[17], resp[18], resp[19]]);
            if err_code != 0 && err_code != -libc::EEXIST {
                return Err(io::Error::from_raw_os_error(-err_code));
            }
        }
        Ok(())
    }

    fn bind_netlink_socket() -> io::Result<OwnedFd> {
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        unsafe {
            let mut sa: libc::sockaddr_nl = mem::zeroed();
            sa.nl_family = libc::AF_NETLINK as u16;
            if libc::bind(
                owned.as_raw_fd(),
                &sa as *const libc::sockaddr_nl as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            ) < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(owned)
    }

    fn attach_tc_ingress_bpf(ifindex: i32, prog_fd: i32) -> io::Result<()> {
        let nl_sock = bind_netlink_socket()?;

        // 1. RTM_NEWQDISC (36) clsact on ifindex
        let mut tcmsg_qdisc = Vec::with_capacity(20);
        tcmsg_qdisc.extend_from_slice(&[0u8, 0, 0, 0]);
        tcmsg_qdisc.extend_from_slice(&ifindex.to_ne_bytes());
        tcmsg_qdisc.extend_from_slice(&0xffff0000u32.to_ne_bytes());
        tcmsg_qdisc.extend_from_slice(&0xfffffff1u32.to_ne_bytes());
        tcmsg_qdisc.extend_from_slice(&0u32.to_ne_bytes());
        tcmsg_qdisc.extend_from_slice(&nla(1, b"clsact\0"));
        let _ = send_netlink_msg(
            nl_sock.as_raw_fd(),
            36,
            0x0001 | 0x0004 | 0x0400 | 0x0100,
            1,
            &tcmsg_qdisc,
        );

        // Remove any prior filter at prio 77 handle 1 first
        let info: u32 = (77u32 << 16) | (0x0003u16.to_be() as u32);
        let mut tcmsg_del = Vec::with_capacity(20);
        tcmsg_del.extend_from_slice(&[0u8, 0, 0, 0]);
        tcmsg_del.extend_from_slice(&ifindex.to_ne_bytes());
        tcmsg_del.extend_from_slice(&1u32.to_ne_bytes());
        tcmsg_del.extend_from_slice(&0xfffffff2u32.to_ne_bytes());
        tcmsg_del.extend_from_slice(&info.to_ne_bytes());
        let _ = send_netlink_msg(nl_sock.as_raw_fd(), 45, 0x0001 | 0x0004, 2, &tcmsg_del);

        // 2. RTM_NEWTFILTER (44) bpf on ingress (parent=0xfffffff2, prio=77, handle=1, proto=ETH_P_ALL)
        let mut tcmsg_filter = Vec::with_capacity(64);
        tcmsg_filter.extend_from_slice(&[0u8, 0, 0, 0]);
        tcmsg_filter.extend_from_slice(&ifindex.to_ne_bytes());
        tcmsg_filter.extend_from_slice(&1u32.to_ne_bytes());
        tcmsg_filter.extend_from_slice(&0xfffffff2u32.to_ne_bytes());
        tcmsg_filter.extend_from_slice(&info.to_ne_bytes());

        let mut bpf_opts = Vec::new();
        bpf_opts.extend_from_slice(&nla(6, &(prog_fd as u32).to_ne_bytes())); // TCA_BPF_FD
        bpf_opts.extend_from_slice(&nla(7, b"rog_direct_tc\0")); // TCA_BPF_NAME
        bpf_opts.extend_from_slice(&nla(8, &1u32.to_ne_bytes())); // TCA_BPF_FLAG_ACT_DIRECT

        tcmsg_filter.extend_from_slice(&nla(1, b"bpf\0")); // TCA_KIND
        tcmsg_filter.extend_from_slice(&nla(2, &bpf_opts)); // TCA_OPTIONS

        send_netlink_msg(
            nl_sock.as_raw_fd(),
            44,
            0x0001 | 0x0004 | 0x0400 | 0x0200 | 0x0100,
            3,
            &tcmsg_filter,
        )
    }

    fn detach_tc_ingress_bpf(ifindex: i32) {
        let Ok(nl_sock) = bind_netlink_socket() else {
            return;
        };
        let info: u32 = (77u32 << 16) | (0x0003u16.to_be() as u32);
        let mut tcmsg_filter = Vec::with_capacity(20);
        tcmsg_filter.extend_from_slice(&[0u8, 0, 0, 0]);
        tcmsg_filter.extend_from_slice(&ifindex.to_ne_bytes());
        tcmsg_filter.extend_from_slice(&1u32.to_ne_bytes());
        tcmsg_filter.extend_from_slice(&0xfffffff2u32.to_ne_bytes());
        tcmsg_filter.extend_from_slice(&info.to_ne_bytes());
        let _ = send_netlink_msg(nl_sock.as_raw_fd(), 45, 0x0001 | 0x0004, 4, &tcmsg_filter);
    }

    fn find_cmd(name: &str) -> String {
        for dir in ["/usr/sbin", "/sbin", "/usr/bin", "/bin"] {
            let p = format!("{dir}/{name}");
            if std::path::Path::new(&p).exists() {
                return p;
            }
        }
        name.to_string()
    }

    fn find_chain_insert_pos(cmd: &str, chain: &str) -> Option<usize> {
        let out = Command::new(cmd)
            .args(["-w", "-t", "mangle", "-S", chain])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut idx = 0usize;
        let mut tproxy_idx = None;
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("-A ") {
                continue;
            }
            idx += 1;
            if trimmed.contains("--mark 0x78/")
                && trimmed.contains("-j RETURN")
                && !trimmed.contains("connmark")
            {
                return Some(idx);
            }
            if tproxy_idx.is_none() && trimmed.contains("-j TPROXY") {
                tproxy_idx = Some(idx);
            }
        }
        tproxy_idx.or(Some(idx + 1))
    }

    fn mangle_rule_exists(cmd: &str, chain: &str, args: &[&str]) -> bool {
        let mut full = vec!["-w", "-t", "mangle", "-C", chain];
        full.extend_from_slice(args);
        Command::new(cmd)
            .args(full)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn mangle_insert_rule(cmd: &str, chain: &str, pos: usize, args: &[&str]) -> bool {
        let pos_str = pos.to_string();
        let mut full = vec!["-w", "-t", "mangle", "-I", chain, pos_str.as_str()];
        full.extend_from_slice(args);
        Command::new(cmd)
            .args(full)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn ensure_mangle_mark_bypass_rule() {
        let _ = Command::new(find_cmd("modprobe"))
            .arg("xt_connmark")
            .output();
        for (bin, chain) in [("iptables", "ROG_TP"), ("ip6tables", "ROG6_TP")] {
            let cmd = find_cmd(bin);
            let Some(mut pos) = find_chain_insert_pos(&cmd, chain) else {
                continue;
            };
            let mark_ret = ["-m", "mark", "--mark", "0x78/0xff", "-j", "RETURN"];
            if !mangle_rule_exists(&cmd, chain, &mark_ret) {
                let _ = mangle_insert_rule(&cmd, chain, pos, &mark_ret);
                pos = find_chain_insert_pos(&cmd, chain).unwrap_or(pos);
            }
            let mark_connmark = [
                "-m",
                "mark",
                "--mark",
                "0x78/0xff",
                "-j",
                "CONNMARK",
                "--set-xmark",
                "0x78/0xff",
            ];
            if !mangle_rule_exists(&cmd, chain, &mark_connmark)
                && !mangle_insert_rule(&cmd, chain, pos, &mark_connmark)
            {
                continue;
            }
            let connmark_ret = ["-m", "connmark", "--mark", "0x78/0xff", "-j", "RETURN"];
            if !mangle_rule_exists(&cmd, chain, &connmark_ret) {
                let _ = mangle_insert_rule(&cmd, chain, pos, &connmark_ret);
            }
        }
    }

    fn is_private_or_local_ip(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_multicast()
                    || v4.is_unspecified()
                    || crate::listener::tproxy::is_local_host_ip(&IpAddr::V4(v4))
            }
            IpAddr::V6(v6) => {
                if let Some(v4) = v6.to_ipv4_mapped() {
                    return is_private_or_local_ip(IpAddr::V4(v4));
                }
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || v6.is_unicast_link_local()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || crate::listener::tproxy::is_local_host_ip(&IpAddr::V6(v6))
            }
        }
    }

    fn encode_cidr_key(cidr: IpNetwork) -> (bool, Vec<u8>) {
        match cidr {
            IpNetwork::V4(v4) => {
                let mut key = Vec::with_capacity(8);
                key.extend_from_slice(&(v4.prefix() as u32).to_ne_bytes());
                key.extend_from_slice(&v4.network().octets());
                (true, key)
            }
            IpNetwork::V6(v6) => {
                let mut key = Vec::with_capacity(20);
                key.extend_from_slice(&(v6.prefix() as u32).to_ne_bytes());
                key.extend_from_slice(&v6.network().octets());
                (false, key)
            }
        }
    }

    pub fn sync_direct_cidrs(cidrs: &[IpNetwork]) {
        if !DIRECT_ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let Some(state) = DIRECT_STATE.get() else {
            return;
        };
        let Ok(mut active) = state.active_cidrs.lock() else {
            return;
        };
        let new_set: HashSet<IpNetwork> = cidrs
            .iter()
            .copied()
            .filter(|c| !is_private_or_local_ip(c.network()))
            .collect();

        for old_cidr in active.difference(&new_set) {
            let (is_v4, key) = encode_cidr_key(*old_cidr);
            let fd = if is_v4 {
                state.v4_lpm.as_raw_fd()
            } else {
                state.v6_lpm.as_raw_fd()
            };
            let _ = map_delete_elem(fd, &key);
        }

        let mut added = 0usize;
        for new_cidr in new_set.difference(&active) {
            let (is_v4, key) = encode_cidr_key(*new_cidr);
            let fd = if is_v4 {
                state.v4_lpm.as_raw_fd()
            } else {
                state.v6_lpm.as_raw_fd()
            };
            if map_update_elem(fd, &key, &[1u8], 0).is_ok() {
                added += 1;
            }
        }

        *active = new_set;
        if added > 0 {
            info!(
                "ebpf_direct: synced {} direct CIDR prefixes into kernel LPM_TRIE maps",
                active.len()
            );
        }
    }

    pub fn notify_cidr_refresh() {
        if let Some(notify) = CIDR_REFRESH_NOTIFY.get() {
            notify.notify_one();
        }
    }

    pub fn disable_ebpf_offload() {
        DIRECT_DISABLED.store(true, Ordering::SeqCst);
        DIRECT_ENABLED.store(false, Ordering::SeqCst);
    }

    struct RingBufMapping {
        consumer_ptr: *mut libc::c_void,
        producer_ptr: *mut libc::c_void,
        page_size: usize,
        ring_size: usize,
    }

    unsafe impl Send for RingBufMapping {}

    impl RingBufMapping {
        fn new(ring_fd: i32, ring_size: usize) -> io::Result<Self> {
            let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            let page_size = usize::try_from(ps).unwrap_or(4096).max(4096);
            if ring_size == 0 || !ring_size.is_power_of_two() || ring_size % page_size != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid ringbuf size",
                ));
            }
            let consumer_ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    page_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    ring_fd,
                    0,
                )
            };
            if consumer_ptr == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            let producer_ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    page_size + 2 * ring_size,
                    libc::PROT_READ,
                    libc::MAP_SHARED,
                    ring_fd,
                    page_size as libc::off_t,
                )
            };
            if producer_ptr == libc::MAP_FAILED {
                let err = io::Error::last_os_error();
                unsafe {
                    libc::munmap(consumer_ptr, page_size);
                }
                return Err(err);
            }
            Ok(Self {
                consumer_ptr,
                producer_ptr,
                page_size,
                ring_size,
            })
        }

        fn poll_events(&self) -> Vec<DirectFlowEvent> {
            let mut out = Vec::new();
            let mask = self.ring_size - 1;
            let cons_atom = unsafe { &*(self.consumer_ptr as *const AtomicU64) };
            let prod_atom = unsafe { &*(self.producer_ptr as *const AtomicU64) };
            let data_base = unsafe { (self.producer_ptr as *const u8).add(self.page_size) };

            let mut cons_pos = cons_atom.load(Ordering::Relaxed);
            let prod_pos = prod_atom.load(Ordering::Acquire);
            while cons_pos < prod_pos {
                let off = (cons_pos as usize) & mask;
                let hdr_ptr = unsafe { data_base.add(off) };
                let len = unsafe { (*(hdr_ptr as *const std::sync::atomic::AtomicU32)).load(Ordering::Acquire) };
                if (len & BPF_RINGBUF_BUSY_BIT) != 0 {
                    break;
                }
                let data_len = (len & 0x3fff_ffff) as usize;
                let stride = (BPF_RINGBUF_HDR_SZ + ((data_len + 7) & !7)) as u64;
                if (len & BPF_RINGBUF_DISCARD_BIT) == 0
                    && data_len >= std::mem::size_of::<DirectFlowEvent>()
                {
                    let ev = unsafe {
                        std::ptr::read_unaligned(
                            hdr_ptr.add(BPF_RINGBUF_HDR_SZ) as *const DirectFlowEvent
                        )
                    };
                    out.push(ev);
                }
                cons_pos = cons_pos.saturating_add(stride);
            }
            cons_atom.store(cons_pos, Ordering::Release);
            out
        }
    }

    impl Drop for RingBufMapping {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.producer_ptr, self.page_size + 2 * self.ring_size);
                libc::munmap(self.consumer_ptr, self.page_size);
            }
        }
    }

    struct TrackedDirectFlow {
        observe: ObserveConnection,
        last_seen: Instant,
        is_tcp: bool,
    }

    fn handle_flow_events(
        events: Vec<DirectFlowEvent>,
        active_flows: &mut HashMap<DirectFlowKey, TrackedDirectFlow>,
        router: &crate::router::DefaultRouter,
        listener_name: &str,
        router_name: &str,
        direct_connectors: &HashSet<String>,
        observe_registry: &ObserveRegistry,
        dns_server_enabled: bool,
    ) {
        let now = Instant::now();
        for ev in events {
            let Some((proto, src_ip, src_port, dst_ip, dst_port)) = ev.key.to_tuple() else {
                continue;
            };
            if dns_server_enabled && proto == 17 && dst_port == 53 {
                continue;
            }
            if is_private_or_local_ip(dst_ip) {
                continue;
            }
            match ev.kind {
                DIRECT_FLOW_EVENT_OPEN => {
                    let proto_str = if proto == 17 { "udp" } else { "tcp" };
                    let (selected, domains, matched_domain) =
                        router.lookup_ebpf_direct_route(router_name, src_ip, dst_ip);
                    let outbound = if direct_connectors.contains(selected.as_str()) {
                        selected
                    } else {
                        direct_connectors
                            .iter()
                            .next()
                            .cloned()
                            .unwrap_or(selected)
                    };
                    info!(
                        "Route listener: {} router: {} proto: {} src_ip: {} src_port: {} dst_ip: {} dst_port: {} domains: {:?} -> {}",
                        listener_name,
                        router_name,
                        proto_str,
                        src_ip,
                        src_port,
                        dst_ip,
                        dst_port,
                        domains,
                        outbound
                    );
                    let site = matched_domain.or_else(|| domains.first().cloned());
                    let obs = observe_registry.open(ConnectionMeta {
                        service: "rog".to_string(),
                        network: proto_str.to_string(),
                        listener: listener_name.to_string(),
                        router: Some(router_name.to_string()),
                        route: Some(outbound.clone()),
                        inbound: Some("ebpf_direct".to_string()),
                        outbound: Some(outbound),
                        source: crate::util::format_endpoint(&src_ip.to_string(), src_port),
                        destination: crate::util::format_endpoint(&dst_ip.to_string(), dst_port),
                        site,
                    });
                    active_flows.insert(
                        ev.key,
                        TrackedDirectFlow {
                            observe: obs,
                            last_seen: now,
                            is_tcp: proto == 6,
                        },
                    );
                }
                DIRECT_FLOW_EVENT_CLOSE => {
                    if let Some(flow) = active_flows.remove(&ev.key) {
                        flow.observe.finish();
                    }
                }
                _ => {}
            }
        }
    }

    fn reap_idle_flows(active_flows: &mut HashMap<DirectFlowKey, TrackedDirectFlow>) {
        if active_flows.is_empty() {
            return;
        }
        let flow_seen_fd = DIRECT_STATE
            .get()
            .and_then(|s| s.flow_seen.as_ref())
            .map(|fd| fd.as_raw_fd());
        let now_ns = monotonic_ns();
        let now = Instant::now();
        let tcp_idle_ns = 180_000_000_000u64;
        let udp_idle_ns = 60_000_000_000u64;

        active_flows.retain(|key, entry| {
            let idle_limit_ns = if entry.is_tcp { tcp_idle_ns } else { udp_idle_ns };
            let idle_limit_dur = Duration::from_nanos(idle_limit_ns);
            let keep = if let (Some(fd), true) = (flow_seen_fd, now_ns > 0) {
                let mut val_bytes = [0u8; 8];
                if map_lookup_elem(fd, key.as_bytes(), &mut val_bytes).is_ok() {
                    let state_ns = u64::from_ne_bytes(val_bytes);
                    if (state_ns & (1u64 << 63)) != 0 {
                        false
                    } else if now_ns.saturating_sub(state_ns) <= idle_limit_ns {
                        entry.last_seen = now;
                        true
                    } else {
                        let _ = map_delete_elem(fd, key.as_bytes());
                        false
                    }
                } else {
                    false
                }
            } else {
                now.saturating_duration_since(entry.last_seen) <= idle_limit_dur
            };
            if !keep {
                entry.observe.finish();
            }
            keep
        });
    }

    pub fn enable_ebpf_offload(
        router: Arc<crate::router::DefaultRouter>,
        listener_name: String,
        router_name: String,
        direct_connectors: HashSet<String>,
        observe_registry: ObserveRegistry,
        dns_server_enabled: bool,
        shutdown: CancellationToken,
    ) {
        if DIRECT_DISABLED.load(Ordering::SeqCst) {
            return;
        }
        if DIRECT_ENABLED.swap(true, Ordering::SeqCst) {
            return;
        }

        let Some((ifname, ifindex)) = detect_default_ifname() else {
            DIRECT_ENABLED.store(false, Ordering::SeqCst);
            return;
        };

        let ring_size: usize = 256 * 1024;
        let init_res = (|| -> io::Result<Option<(OwnedFd, RingBufMapping)>> {
            let v4_lru = create_bpf_map(BPF_MAP_TYPE_LRU_HASH, 4, 8, 32768, 0)?;
            let v4_lpm = create_bpf_map(BPF_MAP_TYPE_LPM_TRIE, 8, 1, 16384, BPF_F_NO_PREALLOC)?;
            let v6_lru = create_bpf_map(BPF_MAP_TYPE_LRU_HASH, 16, 8, 32768, 0)?;
            let v6_lpm = create_bpf_map(BPF_MAP_TYPE_LPM_TRIE, 20, 1, 16384, BPF_F_NO_PREALLOC)?;

            let mut flow_seen_opt: Option<OwnedFd> = None;
            let mut ringbuf_pair: Option<(OwnedFd, RingBufMapping)> = None;
            let prog = match (|| -> io::Result<(OwnedFd, OwnedFd, RingBufMapping, OwnedFd)> {
                let flow_seen = create_bpf_map(
                    BPF_MAP_TYPE_LRU_HASH,
                    std::mem::size_of::<DirectFlowKey>() as u32,
                    8,
                    32768,
                    0,
                )?;
                let ring_fd =
                    create_bpf_map(BPF_MAP_TYPE_RINGBUF, 0, 0, ring_size as u32, 0)?;
                let mapping = RingBufMapping::new(ring_fd.as_raw_fd(), ring_size)?;
                let bytecode = build_tc_direct_prog_with_flow_log_bytecode(
                    v4_lru.as_raw_fd(),
                    v4_lpm.as_raw_fd(),
                    v6_lru.as_raw_fd(),
                    v6_lpm.as_raw_fd(),
                    flow_seen.as_raw_fd(),
                    ring_fd.as_raw_fd(),
                );
                let prog = load_bpf_prog(BPF_PROG_TYPE_SCHED_CLS, &bytecode)?;
                Ok((flow_seen, ring_fd, mapping, prog))
            })() {
                Ok((flow_seen, ring_fd, mapping, prog)) => {
                    flow_seen_opt = Some(flow_seen);
                    ringbuf_pair = Some((ring_fd, mapping));
                    prog
                }
                Err(e) => {
                    warn!(
                        "ebpf_direct: ringbuf flow logging unavailable ({}), using mark-only TC program",
                        e
                    );
                    let bytecode = build_tc_direct_prog_bytecode(
                        v4_lru.as_raw_fd(),
                        v4_lpm.as_raw_fd(),
                        v6_lru.as_raw_fd(),
                        v6_lpm.as_raw_fd(),
                    );
                    load_bpf_prog(BPF_PROG_TYPE_SCHED_CLS, &bytecode)?
                }
            };

            attach_tc_ingress_bpf(ifindex, prog.as_raw_fd())?;
            ensure_mangle_mark_bypass_rule();

            let _ = DIRECT_STATE.set(DirectBpfState {
                v4_lru,
                v4_lpm,
                v6_lru,
                v6_lpm,
                flow_seen: flow_seen_opt,
                _prog: prog,
                active_cidrs: Mutex::new(HashSet::new()),
            });
            Ok(ringbuf_pair)
        })();

        let ringbuf_pair = match init_res {
            Ok(pair) => pair,
            Err(e) => {
                DIRECT_ENABLED.store(false, Ordering::SeqCst);
                warn!(
                    "ebpf_direct: kernel TC eBPF offload unavailable on {} ({}), falling back to userspace Direct",
                    ifname, e
                );
                return;
            }
        };

        let notify = CIDR_REFRESH_NOTIFY
            .get_or_init(|| Arc::new(Notify::new()))
            .clone();
        let initial_cidrs = router.direct_cidrs(&router_name, &direct_connectors);
        sync_direct_cidrs(&initial_cidrs);

        info!(
            "ebpf_direct: attached TC ingress eBPF fast-path on {} (ifindex={}, mark=0x78, flow_log={}, connectors={:?}, static_cidrs={})",
            ifname,
            ifindex,
            ringbuf_pair.is_some(),
            direct_connectors,
            initial_cidrs.len()
        );

        let router_cidr = router.clone();
        let router_name_cidr = router_name.clone();
        let direct_connectors_cidr = direct_connectors.clone();
        let shutdown_cidr = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_cidr.cancelled() => {
                        detach_tc_ingress_bpf(ifindex);
                        info!("ebpf_direct: detached TC ingress eBPF fast-path from {}", ifname);
                        break;
                    }
                    _ = notify.notified() => {
                        let cidrs = router_cidr.direct_cidrs(&router_name_cidr, &direct_connectors_cidr);
                        sync_direct_cidrs(&cidrs);
                    }
                }
            }
        });

        if let Some((ring_fd, mapping)) = ringbuf_pair {
            tokio::spawn(async move {
                let mut active_flows: HashMap<DirectFlowKey, TrackedDirectFlow> = HashMap::new();
                let mut reap_tick = tokio::time::interval(Duration::from_secs(15));
                reap_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                match AsyncFd::new(ring_fd) {
                    Ok(async_ring) => loop {
                        tokio::select! {
                            biased;
                            _ = shutdown.cancelled() => {
                                active_flows.clear();
                                break;
                            }
                            guard_res = async_ring.readable() => {
                                match guard_res {
                                    Ok(mut guard) => {
                                        let events = mapping.poll_events();
                                        guard.clear_ready();
                                        if !events.is_empty() {
                                            handle_flow_events(
                                                events,
                                                &mut active_flows,
                                                &router,
                                                &listener_name,
                                                &router_name,
                                                &direct_connectors,
                                                &observe_registry,
                                                dns_server_enabled,
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        warn!("ebpf_direct: ringbuf AsyncFd error: {}", e);
                                        break;
                                    }
                                }
                            }
                            _ = reap_tick.tick() => {
                                let events = mapping.poll_events();
                                if !events.is_empty() {
                                    handle_flow_events(
                                        events,
                                        &mut active_flows,
                                        &router,
                                        &listener_name,
                                        &router_name,
                                        &direct_connectors,
                                        &observe_registry,
                                        dns_server_enabled,
                                    );
                                }
                                reap_idle_flows(&mut active_flows);
                            }
                        }
                    },
                    Err(e) => {
                        warn!(
                            "ebpf_direct: ringbuf AsyncFd unavailable ({}), using periodic polling",
                            e
                        );
                        let mut poll_tick = tokio::time::interval(Duration::from_millis(100));
                        poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tokio::select! {
                                biased;
                                _ = shutdown.cancelled() => {
                                    active_flows.clear();
                                    break;
                                }
                                _ = poll_tick.tick() => {
                                    let events = mapping.poll_events();
                                    if !events.is_empty() {
                                        handle_flow_events(
                                            events,
                                            &mut active_flows,
                                            &router,
                                            &listener_name,
                                            &router_name,
                                            &direct_connectors,
                                            &observe_registry,
                                            dns_server_enabled,
                                        );
                                    }
                                }
                                _ = reap_tick.tick() => {
                                    reap_idle_flows(&mut active_flows);
                                }
                            }
                        }
                    }
                }
            });
        }
    }

    pub fn note_direct_flow(
        udp: bool,
        src_ip: IpAddr,
        src_port: u16,
        dst_ip: IpAddr,
        dst_port: u16,
    ) {
        if src_port == 0 || dst_port == 0 || !DIRECT_ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let Some(flow_seen) = DIRECT_STATE.get().and_then(|s| s.flow_seen.as_ref()) else {
            return;
        };
        let Some(key) = DirectFlowKey::from_tuple(udp, src_ip, src_port, dst_ip, dst_port) else {
            return;
        };
        let now_ns = monotonic_ns();
        if now_ns == 0 {
            return;
        }
        let _ = map_update_elem(flow_seen.as_raw_fd(), key.as_bytes(), &now_ns.to_ne_bytes(), 0);
    }

    pub fn offload_direct_ip(ip: IpAddr, ttl: Duration) {
        if !DIRECT_ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let Some(state) = DIRECT_STATE.get() else {
            return;
        };
        if is_private_or_local_ip(ip) {
            return;
        }
        let now_ns = monotonic_ns();
        if now_ns == 0 {
            return;
        }
        let expire_ns = now_ns.saturating_add(ttl.as_nanos().min(u64::MAX as u128) as u64);
        let val = expire_ns.to_ne_bytes();

        match ip {
            IpAddr::V4(v4) => {
                let _ = map_update_elem(state.v4_lru.as_raw_fd(), &v4.octets(), &val, 0);
            }
            IpAddr::V6(v6) => {
                if let Some(v4) = v6.to_ipv4_mapped() {
                    let _ = map_update_elem(state.v4_lru.as_raw_fd(), &v4.octets(), &val, 0);
                } else {
                    let _ = map_update_elem(state.v6_lru.as_raw_fd(), &v6.octets(), &val, 0);
                }
            }
        }
    }

    pub fn remove_direct_ip(ip: IpAddr) {
        if !DIRECT_ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let Some(state) = DIRECT_STATE.get() else {
            return;
        };
        match ip {
            IpAddr::V4(v4) => {
                let _ = map_delete_elem(state.v4_lru.as_raw_fd(), &v4.octets());
            }
            IpAddr::V6(v6) => {
                if let Some(v4) = v6.to_ipv4_mapped() {
                    let _ = map_delete_elem(state.v4_lru.as_raw_fd(), &v4.octets());
                } else {
                    let _ = map_delete_elem(state.v6_lru.as_raw_fd(), &v6.octets());
                }
            }
        }
    }

    pub async fn relay_tcp_streams(
        client_tcp: &mut TcpStream,
        server_tcp: &mut TcpStream,
        observe: Option<ObserveConnection>,
    ) -> io::Result<()> {
        let (rx, tx) =
            tokio::io::copy_bidirectional_with_sizes(client_tcp, server_tcp, 16384, 16384).await?;
        if let Some(obs) = &observe {
            obs.add_rx(rx);
            obs.add_tx(tx);
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub use linux_impl::{
    disable_ebpf_offload, enable_ebpf_offload, note_direct_flow, notify_cidr_refresh,
    offload_direct_ip, relay_tcp_streams, remove_direct_ip, sync_direct_cidrs,
};

#[cfg(not(target_os = "linux"))]
pub fn disable_ebpf_offload() {}

#[cfg(not(target_os = "linux"))]
pub fn enable_ebpf_offload(
    _router: Arc<crate::router::DefaultRouter>,
    _listener_name: String,
    _router_name: String,
    _direct_connectors: HashSet<String>,
    _observe_registry: ObserveRegistry,
    _dns_server_enabled: bool,
    _shutdown: CancellationToken,
) {
}

#[cfg(not(target_os = "linux"))]
pub fn note_direct_flow(
    _udp: bool,
    _src_ip: IpAddr,
    _src_port: u16,
    _dst_ip: IpAddr,
    _dst_port: u16,
) {
}

#[cfg(not(target_os = "linux"))]
pub fn notify_cidr_refresh() {}

#[cfg(not(target_os = "linux"))]
pub fn offload_direct_ip(_ip: IpAddr, _ttl: Duration) {}

#[cfg(not(target_os = "linux"))]
pub fn remove_direct_ip(_ip: IpAddr) {}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub fn sync_direct_cidrs(_cidrs: &[IpNetwork]) {}

#[cfg(not(target_os = "linux"))]
pub async fn relay_tcp_streams(
    client_tcp: &mut TcpStream,
    server_tcp: &mut TcpStream,
    observe: Option<ObserveConnection>,
) -> io::Result<()> {
    let (rx, tx) =
        tokio::io::copy_bidirectional_with_sizes(client_tcp, server_tcp, 16384, 16384).await?;
    if let Some(obs) = &observe {
        obs.add_rx(rx);
        obs.add_tx(tx);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ebpf_bytecode_generation() {
        let tc_code = build_tc_direct_prog_bytecode(3, 4, 5, 6);
        assert_eq!(tc_code.len() % 8, 0);
        assert_eq!(tc_code.len() / 8, 64);
        // Last instruction must be exit (0x95)
        assert_eq!(tc_code[tc_code.len() - 8], 0x95);

        let tc_flow_code = build_tc_direct_prog_with_flow_log_bytecode(3, 4, 5, 6, 8, 9);
        assert_eq!(tc_flow_code.len() % 8, 0);
        assert!(tc_flow_code.len() / 8 > 140);
        assert_eq!(tc_flow_code[tc_flow_code.len() - 8], 0x95);

        let sh_code = build_sockhash_verdict_bytecode(7);
        assert_eq!(sh_code.len() % 8, 0);
        assert_eq!(sh_code.len() / 8, 11);
        assert_eq!(sh_code[sh_code.len() - 8], 0x95);
    }

    #[test]
    fn test_direct_flow_key_and_ringbuf_decode() {
        assert_eq!(std::mem::size_of::<DirectFlowKey>(), 40);
        assert_eq!(std::mem::size_of::<DirectFlowEvent>(), 48);

        let k4 = DirectFlowKey::from_tuple(
            false,
            "::ffff:192.168.0.48".parse().unwrap(),
            51234,
            "220.181.38.148".parse().unwrap(),
            443,
        )
        .unwrap();
        assert_eq!(
            k4.to_tuple().unwrap(),
            (
                6,
                "192.168.0.48".parse().unwrap(),
                51234,
                "220.181.38.148".parse().unwrap(),
                443
            )
        );

        let k6 = DirectFlowKey::from_tuple(
            true,
            "240e:305::1".parse().unwrap(),
            60000,
            "2400:3200::1".parse().unwrap(),
            443,
        )
        .unwrap();
        assert_eq!(
            k6.to_tuple().unwrap(),
            (
                17,
                "240e:305::1".parse().unwrap(),
                60000,
                "2400:3200::1".parse().unwrap(),
                443
            )
        );

        let ev_open = DirectFlowEvent {
            key: k4,
            kind: DIRECT_FLOW_EVENT_OPEN,
            tcp_flags: 0x02,
            _pad: [0; 6],
        };
        let ev_close = DirectFlowEvent {
            key: k6,
            kind: DIRECT_FLOW_EVENT_CLOSE,
            tcp_flags: 0,
            _pad: [0; 6],
        };

        // Construct a 256-byte ring starting at pos=168 containing:
        // OPEN (168..224) + discarded record (224..240) + wrap-around CLOSE (240..296), then BUSY at 296.
        let mut ring = vec![0u8; 256];
        let write_rec = |ring: &mut [u8], pos: usize, hdr_len: u32, payload: &[u8]| {
            let off = pos & (ring.len() - 1);
            ring[off..off + 4].copy_from_slice(&hdr_len.to_ne_bytes());
            for (i, b) in payload.iter().enumerate() {
                ring[(off + BPF_RINGBUF_HDR_SZ + i) & (ring.len() - 1)] = *b;
            }
        };

        let open_bytes = unsafe {
            std::slice::from_raw_parts(
                (&ev_open as *const DirectFlowEvent) as *const u8,
                std::mem::size_of::<DirectFlowEvent>(),
            )
        };
        let close_bytes = unsafe {
            std::slice::from_raw_parts(
                (&ev_close as *const DirectFlowEvent) as *const u8,
                std::mem::size_of::<DirectFlowEvent>(),
            )
        };

        // Record 0 at pos=168 (56B stride -> next=224)
        write_rec(&mut ring, 168, 48, open_bytes);
        // Record 1 at pos=224 discarded (len=8 -> 16B stride -> next=240)
        write_rec(&mut ring, 224, 8 | BPF_RINGBUF_DISCARD_BIT, &[0u8; 8]);
        // Record 2 at pos=240 (wraps around 256 boundary! 56B stride -> next=296)
        write_rec(&mut ring, 240, 48, close_bytes);
        // Record 3 at pos=296 busy (must stop here)
        write_rec(&mut ring, 296, 48 | BPF_RINGBUF_BUSY_BIT, &[0u8; 48]);

        let (new_cons, decoded) = decode_ringbuf_records(&ring, 168, 352);
        assert_eq!(new_cons, 296);
        assert_eq!(decoded, vec![ev_open, ev_close]);
    }
}

use ipnetwork::IpNetwork;
use proxy_observe::ObserveConnection;
use std::collections::HashSet;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
#[cfg(target_os = "linux")]
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

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

    let mut items = Vec::with_capacity(64);
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
    use std::mem;
    use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use tokio::sync::Notify;

    const BPF_MAP_CREATE: libc::c_int = 0;
    const BPF_MAP_UPDATE_ELEM: libc::c_int = 2;
    const BPF_MAP_DELETE_ELEM: libc::c_int = 3;
    const BPF_PROG_LOAD: libc::c_int = 5;
    const BPF_PROG_ATTACH: libc::c_int = 8;

    const BPF_MAP_TYPE_LRU_HASH: u32 = 9;
    const BPF_MAP_TYPE_LPM_TRIE: u32 = 11;
    const BPF_MAP_TYPE_SOCKHASH: u32 = 18;

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

    fn attach_tc_ingress_bpf(ifindex: i32, prog_fd: i32) -> io::Result<()> {
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
        let nl_sock = unsafe { OwnedFd::from_raw_fd(fd) };

        // 1. RTM_NEWQDISC (36) clsact on ifindex
        let mut tcmsg_qdisc = Vec::with_capacity(20);
        tcmsg_qdisc.extend_from_slice(&[0u8, 0, 0, 0]);
        tcmsg_qdisc.extend_from_slice(&ifindex.to_ne_bytes());
        tcmsg_qdisc.extend_from_slice(&0xffff0000u32.to_ne_bytes());
        tcmsg_qdisc.extend_from_slice(&0xfffffff1u32.to_ne_bytes());
        tcmsg_qdisc.extend_from_slice(&0u32.to_ne_bytes());
        tcmsg_qdisc.extend_from_slice(&nla(1, b"clsact\0"));
        // NLM_F_REQUEST(1) | NLM_F_ACK(4) | NLM_F_CREATE(0x400) | NLM_F_EXCL(0x200)
        let _ = send_netlink_msg(
            nl_sock.as_raw_fd(),
            36,
            0x0001 | 0x0004 | 0x0400,
            1,
            &tcmsg_qdisc,
        );

        // 2. RTM_NEWTFILTER (44) bpf on ingress (parent=0xfffffff2, prio=77, handle=1, proto=ETH_P_ALL)
        let info: u32 = (77u32 << 16) | (0x0003u16.to_be() as u32);
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

        // NLM_F_REQUEST(1) | NLM_F_ACK(4) | NLM_F_CREATE(0x400) | NLM_F_REPLACE(0x100)
        send_netlink_msg(
            nl_sock.as_raw_fd(),
            44,
            0x0001 | 0x0004 | 0x0400 | 0x0100,
            2,
            &tcmsg_filter,
        )
    }

    fn detach_tc_ingress_bpf(ifindex: i32) {
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        };
        if fd < 0 {
            return;
        }
        let nl_sock = unsafe { OwnedFd::from_raw_fd(fd) };
        let info: u32 = (77u32 << 16) | (0x0003u16.to_be() as u32);
        let mut tcmsg_filter = Vec::with_capacity(20);
        tcmsg_filter.extend_from_slice(&[0u8, 0, 0, 0]);
        tcmsg_filter.extend_from_slice(&ifindex.to_ne_bytes());
        tcmsg_filter.extend_from_slice(&1u32.to_ne_bytes());
        tcmsg_filter.extend_from_slice(&0xfffffff2u32.to_ne_bytes());
        tcmsg_filter.extend_from_slice(&info.to_ne_bytes());
        let _ = send_netlink_msg(nl_sock.as_raw_fd(), 45, 0x0001 | 0x0004, 3, &tcmsg_filter);
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

    fn configure_mangle_mark_bypass(enable: bool) {
        let rules: [&[&str]; 3] = [
            &[
                "-m",
                "socket",
                "--transparent",
                "-j",
                "MARK",
                "--set-xmark",
                "0x77/0xff",
            ],
            &["-m", "socket", "--transparent", "-j", "ACCEPT"],
            &["-m", "mark", "--mark", "0x78/0xff", "-j", "RETURN"],
        ];
        for (bin, chain) in [("iptables", "ROG_TP"), ("ip6tables", "ROG6_TP")] {
            let cmd = find_cmd(bin);
            // Always remove existing copies first so order (1, 2, 3) is deterministic
            for rule in &rules {
                let mut args = vec!["-w", "-t", "mangle", "-D", chain];
                args.extend_from_slice(rule);
                let _ = Command::new(&cmd).args(&args).output();
            }
            if enable {
                for (idx, rule) in rules.iter().enumerate() {
                    let pos = (idx + 1).to_string();
                    let mut args = vec!["-w", "-t", "mangle", "-I", chain, pos.as_str()];
                    args.extend_from_slice(rule);
                    let _ = Command::new(&cmd).args(&args).output();
                }
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
        let new_set: HashSet<IpNetwork> = cidrs.iter().copied().collect();

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

    pub fn enable_ebpf_offload(
        router: Arc<crate::router::DefaultRouter>,
        router_name: String,
        direct_connectors: HashSet<String>,
        shutdown: CancellationToken,
    ) {
        if DIRECT_DISABLED.load(Ordering::SeqCst) {
            return;
        }
        if DIRECT_ENABLED.swap(true, Ordering::SeqCst) {
            return;
        }

        // Also warm up SOCKHASH for zero-copy TCP splicing
        let _ = init_sockhash();

        let Some((ifname, ifindex)) = detect_default_ifname() else {
            DIRECT_ENABLED.store(false, Ordering::SeqCst);
            return;
        };

        let init_res = (|| -> io::Result<()> {
            let v4_lru = create_bpf_map(BPF_MAP_TYPE_LRU_HASH, 4, 8, 32768, 0)?;
            let v4_lpm = create_bpf_map(BPF_MAP_TYPE_LPM_TRIE, 8, 1, 16384, BPF_F_NO_PREALLOC)?;
            let v6_lru = create_bpf_map(BPF_MAP_TYPE_LRU_HASH, 16, 8, 32768, 0)?;
            let v6_lpm = create_bpf_map(BPF_MAP_TYPE_LPM_TRIE, 20, 1, 16384, BPF_F_NO_PREALLOC)?;

            let bytecode = build_tc_direct_prog_bytecode(
                v4_lru.as_raw_fd(),
                v4_lpm.as_raw_fd(),
                v6_lru.as_raw_fd(),
                v6_lpm.as_raw_fd(),
            );
            let prog = load_bpf_prog(BPF_PROG_TYPE_SCHED_CLS, &bytecode)?;
            attach_tc_ingress_bpf(ifindex, prog.as_raw_fd())?;
            configure_mangle_mark_bypass(true);

            let _ = DIRECT_STATE.set(DirectBpfState {
                v4_lru,
                v4_lpm,
                v6_lru,
                v6_lpm,
                _prog: prog,
                active_cidrs: Mutex::new(HashSet::new()),
            });
            Ok(())
        })();

        if let Err(e) = init_res {
            DIRECT_ENABLED.store(false, Ordering::SeqCst);
            debug!(
                "ebpf_direct: kernel TC eBPF offload unavailable on {} ({}), falling back to userspace Direct",
                ifname, e
            );
            return;
        }

        info!(
            "ebpf_direct: attached TC ingress eBPF fast-path on {} (ifindex={}, mark=0x78)",
            ifname, ifindex
        );

        let notify = CIDR_REFRESH_NOTIFY
            .get_or_init(|| Arc::new(Notify::new()))
            .clone();
        let initial_cidrs = router.direct_cidrs(&router_name, &direct_connectors);
        sync_direct_cidrs(&initial_cidrs);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => {
                        detach_tc_ingress_bpf(ifindex);
                        configure_mangle_mark_bypass(false);
                        info!("ebpf_direct: detached TC ingress eBPF fast-path from {}", ifname);
                        break;
                    }
                    _ = notify.notified() => {
                        let cidrs = router.direct_cidrs(&router_name, &direct_connectors);
                        sync_direct_cidrs(&cidrs);
                    }
                }
            }
        });
    }

    pub fn offload_direct_ip(ip: IpAddr, ttl: Duration) {
        if !DIRECT_ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let Some(state) = DIRECT_STATE.get() else {
            return;
        };
        if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
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

    fn init_sockhash() -> Option<&'static SockHashState> {
        SOCKHASH_STATE
            .get_or_init(|| {
                let map_fd = create_bpf_map(BPF_MAP_TYPE_SOCKHASH, 8, 4, 4096, 0).ok()?;
                let bytecode = build_sockhash_verdict_bytecode(map_fd.as_raw_fd());
                let prog_fd = load_bpf_prog(BPF_PROG_TYPE_SK_SKB, &bytecode).ok()?;
                attach_bpf_prog(
                    map_fd.as_raw_fd(),
                    prog_fd.as_raw_fd(),
                    BPF_SK_SKB_STREAM_VERDICT,
                )
                .ok()?;
                info!("ebpf_sockmap: initialized kernel SOCKHASH stream_verdict zero-copy TCP splicing");
                Some(SockHashState {
                    map_fd,
                    _prog_fd: prog_fd,
                })
            })
            .as_ref()
    }

    fn get_socket_cookie(fd: i32) -> io::Result<u64> {
        unsafe {
            let mut cookie: u64 = 0;
            let mut len = mem::size_of::<u64>() as libc::socklen_t;
            let ret = libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                SO_COOKIE,
                &mut cookie as *mut u64 as *mut libc::c_void,
                &mut len,
            );
            if ret < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(cookie)
            }
        }
    }

    struct SockHashGuard {
        map_fd: i32,
        cookie_a: u64,
        cookie_b: u64,
    }

    impl Drop for SockHashGuard {
        fn drop(&mut self) {
            let _ = map_delete_elem(self.map_fd, &self.cookie_a.to_ne_bytes());
            let _ = map_delete_elem(self.map_fd, &self.cookie_b.to_ne_bytes());
        }
    }

    async fn drain_pending(
        src: &mut TcpStream,
        dst: &mut TcpStream,
        buf: &mut [u8],
        observe: &Option<ObserveConnection>,
        is_tx: bool,
    ) -> io::Result<bool> {
        loop {
            match src.try_read(buf) {
                Ok(0) => return Ok(true),
                Ok(n) => {
                    dst.write_all(&buf[..n]).await?;
                    if let Some(obs) = observe {
                        if is_tx {
                            obs.add_tx(n as u64);
                        } else {
                            obs.add_rx(n as u64);
                        }
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) => return Err(e),
            }
        }
    }

    pub async fn relay_tcp_streams(
        client_tcp: &mut TcpStream,
        server_tcp: &mut TcpStream,
        observe: Option<ObserveConnection>,
    ) -> io::Result<()> {
        if !DIRECT_DISABLED.load(Ordering::Relaxed) {
            if let Some(sh) = init_sockhash() {
                let fd_c = client_tcp.as_raw_fd();
                let fd_s = server_tcp.as_raw_fd();
                if let (Ok(ck_c), Ok(ck_s)) = (get_socket_cookie(fd_c), get_socket_cookie(fd_s)) {
                    let map_fd = sh.map_fd.as_raw_fd();
                    let r1 = map_update_elem(
                        map_fd,
                        &ck_c.to_ne_bytes(),
                        &(fd_s as u32).to_ne_bytes(),
                        0,
                    );
                    let r2 = map_update_elem(
                        map_fd,
                        &ck_s.to_ne_bytes(),
                        &(fd_c as u32).to_ne_bytes(),
                        0,
                    );
                    if r1.is_ok() && r2.is_ok() {
                        let guard = SockHashGuard {
                            map_fd,
                            cookie_a: ck_c,
                            cookie_b: ck_s,
                        };
                        let mut buf = [0u8; 16384];
                        // Drain any bytes that arrived in sk_receive_queue before SOCKHASH registration
                        let c_eof =
                            drain_pending(client_tcp, server_tcp, &mut buf, &observe, false)
                                .await?;
                        let s_eof =
                            drain_pending(server_tcp, client_tcp, &mut buf, &observe, true).await?;

                        if !c_eof && !s_eof {
                            loop {
                                tokio::select! {
                                    res = client_tcp.readable() => {
                                        res?;
                                        if drain_pending(client_tcp, server_tcp, &mut buf, &observe, false).await? {
                                            break;
                                        }
                                    }
                                    res = server_tcp.readable() => {
                                        res?;
                                        if drain_pending(server_tcp, client_tcp, &mut buf, &observe, true).await? {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        drop(guard);
                        // Finish any half-closed tail bytes
                        let _ = tokio::io::copy_bidirectional_with_sizes(
                            client_tcp, server_tcp, 8192, 8192,
                        )
                        .await;
                        return Ok(());
                    } else {
                        let _ = map_delete_elem(map_fd, &ck_c.to_ne_bytes());
                        let _ = map_delete_elem(map_fd, &ck_s.to_ne_bytes());
                    }
                }
            }
        }

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
    disable_ebpf_offload, enable_ebpf_offload, notify_cidr_refresh, offload_direct_ip,
    relay_tcp_streams, remove_direct_ip, sync_direct_cidrs,
};

#[cfg(not(target_os = "linux"))]
pub fn disable_ebpf_offload() {}

#[cfg(not(target_os = "linux"))]
pub fn enable_ebpf_offload(
    _router: Arc<crate::router::DefaultRouter>,
    _router_name: String,
    _direct_connectors: HashSet<String>,
    _shutdown: CancellationToken,
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

        let sh_code = build_sockhash_verdict_bytecode(7);
        assert_eq!(sh_code.len() % 8, 0);
        assert_eq!(sh_code.len() / 8, 11);
        assert_eq!(sh_code[sh_code.len() - 8], 0x95);
    }
}

use crate::def::config::Listener;
use crate::def::{RunStream, RunUdpReader, UDPPacket};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;
use tokio::time::{Instant, timeout_at};

pub(crate) const DEFAULT_SNIFF_TIMEOUT_MS: u64 = 100;
pub(crate) const DEFAULT_SNIFF_PORTS: &[u16] = &[80, 443, 8443];
const MAX_SNIFF_BYTES: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SniffMode {
    Off,
    Ambiguous,
    Always,
}

#[derive(Debug, Clone)]
pub(crate) struct SniffConfig {
    pub mode: SniffMode,
    pub timeout: Duration,
    pub ports: Vec<u16>,
    pub override_destination: bool,
}

impl SniffConfig {
    pub fn from_listener(listener: &Listener) -> Self {
        let mut mode = if listener.proto == "tproxy" {
            SniffMode::Ambiguous
        } else {
            SniffMode::Off
        };
        let mut timeout_ms = DEFAULT_SNIFF_TIMEOUT_MS;
        let mut ports = DEFAULT_SNIFF_PORTS.to_vec();
        let mut override_destination = false;

        if let Some(ref opts) = listener.options {
            if let Some(v) = opts.get("sniff") {
                if let Some(b) = v.as_bool() {
                    mode = if b {
                        SniffMode::Always
                    } else {
                        SniffMode::Off
                    };
                } else if let Some(s) = v.as_str() {
                    match s.trim().to_ascii_lowercase().as_str() {
                        "off" | "false" | "none" | "disabled" => mode = SniffMode::Off,
                        "ambiguous" | "auto" | "conflict" => mode = SniffMode::Ambiguous,
                        "always" | "true" | "all" | "on" => mode = SniffMode::Always,
                        _ => {}
                    }
                }
            }
            if let Some(ms) = opts.get("sniff_timeout_ms").and_then(|v| v.as_integer()) {
                if ms > 0 {
                    timeout_ms = ms as u64;
                }
            }
            if let Some(arr) = opts.get("sniff_ports").and_then(|v| v.as_array()) {
                let parsed: Vec<u16> = arr
                    .iter()
                    .filter_map(|v| v.as_integer())
                    .filter_map(|p| u16::try_from(p).ok())
                    .filter(|&p| p > 0)
                    .collect();
                if !parsed.is_empty() {
                    ports = parsed;
                }
            }
            if let Some(b) = opts
                .get("sniff_override_destination")
                .and_then(|v| v.as_bool())
            {
                override_destination = b;
            }
        }

        Self {
            mode,
            timeout: Duration::from_millis(timeout_ms),
            ports,
            override_destination,
        }
    }

    pub fn supports_port(&self, port: u16) -> bool {
        self.mode != SniffMode::Off && self.ports.contains(&port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseOutcome {
    Found(String),
    CompleteNoHost,
    Incomplete,
    NotMatched,
}

pub(crate) fn normalize_sniffed_host(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('.');
    if trimmed.is_empty() || trimmed.len() > 253 {
        return None;
    }
    let lowered = trimmed.to_ascii_lowercase();
    if lowered.parse::<IpAddr>().is_ok() {
        return None;
    }
    if !lowered.bytes().all(|b| {
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.' || b == b'_'
    }) {
        return None;
    }
    Some(lowered)
}

pub(crate) fn parse_tls_client_hello_handshake_sni(payload: &[u8]) -> ParseOutcome {
    if payload.is_empty() {
        return ParseOutcome::Incomplete;
    }
    if payload[0] != 0x01 {
        return ParseOutcome::NotMatched;
    }
    if payload.len() < 4 {
        return ParseOutcome::Incomplete;
    }
    let hs_len =
        ((payload[1] as usize) << 16) | ((payload[2] as usize) << 8) | (payload[3] as usize);
    if !(38..=16384).contains(&hs_len) {
        return ParseOutcome::NotMatched;
    }
    let hs_complete = payload.len() >= 4 + hs_len;
    let hs_avail = &payload[4..payload.len().min(4 + hs_len)];

    // client_version (2) + random (32) + session_id_len (1)
    if hs_avail.len() < 35 {
        return if hs_complete {
            ParseOutcome::NotMatched
        } else {
            ParseOutcome::Incomplete
        };
    }
    let sid_len = hs_avail[34] as usize;
    let mut pos = 35 + sid_len;

    if pos + 2 > hs_avail.len() {
        return if hs_complete {
            ParseOutcome::NotMatched
        } else {
            ParseOutcome::Incomplete
        };
    }
    let cs_len = u16::from_be_bytes([hs_avail[pos], hs_avail[pos + 1]]) as usize;
    pos += 2 + cs_len;

    if pos + 1 > hs_avail.len() {
        return if hs_complete {
            ParseOutcome::NotMatched
        } else {
            ParseOutcome::Incomplete
        };
    }
    let comp_len = hs_avail[pos] as usize;
    pos += 1 + comp_len;

    if pos > hs_len {
        return ParseOutcome::NotMatched;
    }
    if pos == hs_len {
        return ParseOutcome::CompleteNoHost;
    }
    if pos + 2 > hs_avail.len() {
        return if hs_complete {
            ParseOutcome::NotMatched
        } else {
            ParseOutcome::Incomplete
        };
    }
    let ext_total = u16::from_be_bytes([hs_avail[pos], hs_avail[pos + 1]]) as usize;
    pos += 2;
    let ext_end = (pos + ext_total).min(hs_len);

    while pos + 4 <= ext_end {
        if pos + 4 > hs_avail.len() {
            return if hs_complete {
                ParseOutcome::CompleteNoHost
            } else {
                ParseOutcome::Incomplete
            };
        }
        let ext_type = u16::from_be_bytes([hs_avail[pos], hs_avail[pos + 1]]);
        let ext_len = u16::from_be_bytes([hs_avail[pos + 2], hs_avail[pos + 3]]) as usize;
        pos += 4;
        if pos + ext_len > ext_end {
            return ParseOutcome::NotMatched;
        }
        if pos + ext_len > hs_avail.len() {
            return if hs_complete {
                ParseOutcome::NotMatched
            } else {
                ParseOutcome::Incomplete
            };
        }
        if ext_type == 0x0000 {
            let sni = &hs_avail[pos..pos + ext_len];
            if sni.len() < 2 {
                return ParseOutcome::CompleteNoHost;
            }
            let list_len = u16::from_be_bytes([sni[0], sni[1]]) as usize;
            let mut spos = 2usize;
            let send = (2 + list_len).min(sni.len());
            while spos + 3 <= send {
                let name_type = sni[spos];
                let name_len = u16::from_be_bytes([sni[spos + 1], sni[spos + 2]]) as usize;
                spos += 3;
                if spos + name_len > send {
                    break;
                }
                if name_type == 0x00 {
                    if let Ok(raw_host) = std::str::from_utf8(&sni[spos..spos + name_len]) {
                        if let Some(host) = normalize_sniffed_host(raw_host) {
                            return ParseOutcome::Found(host);
                        }
                    }
                }
                spos += name_len;
            }
            return ParseOutcome::CompleteNoHost;
        }
        pos += ext_len;
    }

    if hs_complete {
        ParseOutcome::CompleteNoHost
    } else {
        ParseOutcome::Incomplete
    }
}

pub(crate) fn parse_tls_client_hello_sni(buf: &[u8]) -> ParseOutcome {
    if buf.is_empty() {
        return ParseOutcome::Incomplete;
    }
    if buf[0] != 0x16 {
        return ParseOutcome::NotMatched;
    }
    if buf.len() >= 2 && buf[1] != 0x03 {
        return ParseOutcome::NotMatched;
    }
    if buf.len() >= 3 && buf[2] > 0x04 {
        return ParseOutcome::NotMatched;
    }
    if buf.len() < 5 {
        return ParseOutcome::Incomplete;
    }
    let record_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    if !(42..=16384).contains(&record_len) {
        return ParseOutcome::NotMatched;
    }
    let end = buf.len().min(5 + record_len);
    match parse_tls_client_hello_handshake_sni(&buf[5..end]) {
        ParseOutcome::NotMatched if buf.len() < 5 + record_len => ParseOutcome::NotMatched,
        other => other,
    }
}

pub(crate) fn parse_http_host(buf: &[u8]) -> ParseOutcome {
    const METHODS: &[&[u8]] = &[
        b"GET ",
        b"POST ",
        b"HEAD ",
        b"PUT ",
        b"DELETE ",
        b"OPTIONS ",
        b"PATCH ",
        b"CONNECT ",
        b"TRACE ",
    ];
    if buf.is_empty() {
        return ParseOutcome::Incomplete;
    }
    let maybe_http = METHODS.iter().any(|m| {
        if buf.len() < m.len() {
            m.starts_with(buf)
        } else {
            buf.starts_with(m)
        }
    });
    if !maybe_http {
        return ParseOutcome::NotMatched;
    }
    if buf.len() < 8 {
        return ParseOutcome::Incomplete;
    }

    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n").map(|p| p + 2));
    let scan_slice = &buf[..header_end.unwrap_or(buf.len())];
    let text = match std::str::from_utf8(scan_slice) {
        Ok(s) => s,
        Err(_) => return ParseOutcome::NotMatched,
    };

    for line in text.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, val)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("host") {
                let raw_val = val.trim();
                let host_only = if let Some(rest) = raw_val.strip_prefix('[') {
                    rest.split_once(']')
                        .map(|(h, _)| h)
                        .unwrap_or(raw_val)
                } else {
                    raw_val
                        .split_once(':')
                        .map(|(h, _)| h)
                        .unwrap_or(raw_val)
                };
                return match normalize_sniffed_host(host_only) {
                    Some(host) => ParseOutcome::Found(host),
                    None => ParseOutcome::CompleteNoHost,
                };
            }
        }
    }

    if header_end.is_some() {
        ParseOutcome::CompleteNoHost
    } else {
        ParseOutcome::Incomplete
    }
}

pub(crate) fn sniff_buffer(buf: &[u8]) -> ParseOutcome {
    let tls = parse_tls_client_hello_sni(buf);
    if tls != ParseOutcome::NotMatched {
        return tls;
    }
    parse_http_host(buf)
}

pub(crate) async fn sniff_tcp_stream(
    stream: &mut dyn RunStream,
    max_wait: Duration,
) -> (Option<String>, Option<Vec<u8>>) {
    let deadline = Instant::now() + max_wait;
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 2048];

    loop {
        let remaining = MAX_SNIFF_BYTES.saturating_sub(buf.len());
        if remaining == 0 {
            break;
        }
        let read_len = remaining.min(tmp.len());
        match timeout_at(deadline, stream.read(&mut tmp[..read_len])).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(n)) => {
                buf.extend_from_slice(&tmp[..n]);
                match sniff_buffer(&buf) {
                    ParseOutcome::Found(host) => return (Some(host), Some(buf)),
                    ParseOutcome::CompleteNoHost | ParseOutcome::NotMatched => {
                        return (None, Some(buf));
                    }
                    ParseOutcome::Incomplete => continue,
                }
            }
        }
    }

    let cache = if buf.is_empty() { None } else { Some(buf) };
    (None, cache)
}

fn read_quic_varint(buf: &[u8], pos: &mut usize) -> Option<usize> {
    let first = *buf.get(*pos)?;
    let len = 1usize << (first >> 6);
    if *pos + len > buf.len() {
        return None;
    }
    let val: u64 = match len {
        1 => (first & 0x3f) as u64,
        2 => (u16::from_be_bytes([buf[*pos], buf[*pos + 1]]) & 0x3fff) as u64,
        4 => (u32::from_be_bytes([buf[*pos], buf[*pos + 1], buf[*pos + 2], buf[*pos + 3]])
            & 0x3fff_ffff) as u64,
        8 => {
            u64::from_be_bytes([
                buf[*pos],
                buf[*pos + 1],
                buf[*pos + 2],
                buf[*pos + 3],
                buf[*pos + 4],
                buf[*pos + 5],
                buf[*pos + 6],
                buf[*pos + 7],
            ]) & 0x3fff_ffff_ffff_ffff
        }
        _ => return None,
    };
    *pos += len;
    usize::try_from(val).ok()
}

fn quic_version_info(ver: u32) -> Option<(rustls::quic::Version, u8, u8)> {
    match ver {
        0x0000_0001 => Some((rustls::quic::Version::V1, 0, 3)),
        0x6b33_43cf => Some((rustls::quic::Version::V2, 1, 0)),
        0xff00_001d..=0xff00_0020 => Some((rustls::quic::Version::V1Draft, 0, 3)),
        _ => None,
    }
}

#[derive(Debug, Default)]
pub(crate) struct QuicInitialSniffer {
    chunks: BTreeMap<usize, Vec<u8>>,
    stored_bytes: usize,
    saw_initial: bool,
}

impl QuicInitialSniffer {
    fn insert_chunk(&mut self, offset: usize, data: &[u8]) {
        if offset >= MAX_SNIFF_BYTES || data.is_empty() || self.stored_bytes >= MAX_SNIFF_BYTES * 2
        {
            return;
        }
        let max_take = (MAX_SNIFF_BYTES - offset).min(data.len());
        let slice = &data[..max_take];
        if let Some(existing) = self.chunks.get(&offset) {
            if existing.len() >= slice.len() {
                return;
            }
            self.stored_bytes = self.stored_bytes.saturating_sub(existing.len());
        }
        self.stored_bytes += slice.len();
        self.chunks.insert(offset, slice.to_vec());
    }

    fn contiguous_prefix(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (&off, data) in &self.chunks {
            if off > out.len() {
                break;
            }
            let overlap = out.len() - off;
            if overlap < data.len() {
                out.extend_from_slice(&data[overlap..]);
            }
        }
        out
    }

    pub fn feed_datagram(&mut self, datagram: &[u8]) -> ParseOutcome {
        let Some(suite) = rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256.tls13()
        else {
            return ParseOutcome::NotMatched;
        };
        let Some(quic_alg) = suite.quic else {
            return ParseOutcome::NotMatched;
        };

        let mut offset = 0usize;
        let mut decrypted_this_datagram = false;
        while offset < datagram.len() {
            let pkt_start = offset;
            let first = datagram[offset];
            // QUIC Long Header (bit 7 = 1, fixed bit 6 = 1)
            if (first & 0xc0) != 0xc0 {
                break;
            }
            if offset + 7 > datagram.len() {
                break;
            }
            let ver = u32::from_be_bytes([
                datagram[offset + 1],
                datagram[offset + 2],
                datagram[offset + 3],
                datagram[offset + 4],
            ]);
            let Some((quic_ver, initial_type, retry_type)) = quic_version_info(ver) else {
                break;
            };
            let pkt_type = (first & 0x30) >> 4;

            let mut cur = offset + 5;
            let dcid_len = *match datagram.get(cur) {
                Some(v) => v,
                None => break,
            } as usize;
            cur += 1;
            if dcid_len > 20 || cur + dcid_len >= datagram.len() {
                break;
            }
            let dcid = &datagram[cur..cur + dcid_len];
            cur += dcid_len;

            let scid_len = *match datagram.get(cur) {
                Some(v) => v,
                None => break,
            } as usize;
            cur += 1;
            if scid_len > 20 || cur + scid_len > datagram.len() {
                break;
            }
            cur += scid_len;

            if pkt_type != initial_type {
                if pkt_type == retry_type {
                    break;
                }
                let Some(len) = read_quic_varint(datagram, &mut cur) else {
                    break;
                };
                if cur + len > datagram.len() {
                    break;
                }
                offset = cur + len;
                continue;
            }

            let Some(token_len) = read_quic_varint(datagram, &mut cur) else {
                break;
            };
            if cur + token_len > datagram.len() {
                break;
            }
            cur += token_len;

            let Some(payload_len) = read_quic_varint(datagram, &mut cur) else {
                break;
            };
            let pn_offset_in_pkt = cur - pkt_start;
            let pkt_end = cur.saturating_add(payload_len);
            if pkt_end > datagram.len() || payload_len < 20 {
                break;
            }

            let keys = rustls::quic::Keys::initial(
                quic_ver,
                suite,
                quic_alg,
                dcid,
                rustls::Side::Server,
            );
            let mut pkt = datagram[pkt_start..pkt_end].to_vec();
            let (hdr_prefix, pn_and_body) = pkt.split_at_mut(pn_offset_in_pkt);
            let (pn_area, rest) = pn_and_body.split_at_mut(4);
            let sample_len = keys.remote.header.sample_len();
            if rest.len() < sample_len {
                break;
            }
            if keys
                .remote
                .header
                .decrypt_in_place(&rest[..sample_len], &mut hdr_prefix[0], pn_area)
                .is_err()
            {
                offset = pkt_end;
                continue;
            }
            let pn_len = ((hdr_prefix[0] & 0x03) as usize) + 1;
            let mut pn = 0u64;
            for &b in &pn_area[..pn_len] {
                pn = (pn << 8) | (b as u64);
            }
            let (aad, cipher) = pkt.split_at_mut(pn_offset_in_pkt + pn_len);
            let Ok(plaintext) = keys.remote.packet.decrypt_in_place(pn, aad, cipher) else {
                offset = pkt_end;
                continue;
            };

            self.saw_initial = true;
            decrypted_this_datagram = true;
            let mut fpos = 0usize;
            while fpos < plaintext.len() {
                let frame_type = plaintext[fpos];
                fpos += 1;
                match frame_type {
                    0x00 | 0x01 => continue, // PADDING / PING
                    0x02 | 0x03 => {
                        // ACK / ACK_ECN
                        if read_quic_varint(plaintext, &mut fpos).is_none()
                            || read_quic_varint(plaintext, &mut fpos).is_none()
                        {
                            break;
                        }
                        let Some(ack_range_cnt) = read_quic_varint(plaintext, &mut fpos) else {
                            break;
                        };
                        if read_quic_varint(plaintext, &mut fpos).is_none() {
                            break;
                        }
                        let mut ok = true;
                        for _ in 0..ack_range_cnt {
                            if read_quic_varint(plaintext, &mut fpos).is_none()
                                || read_quic_varint(plaintext, &mut fpos).is_none()
                            {
                                ok = false;
                                break;
                            }
                        }
                        if !ok {
                            break;
                        }
                        if frame_type == 0x03 {
                            for _ in 0..3 {
                                if read_quic_varint(plaintext, &mut fpos).is_none() {
                                    ok = false;
                                    break;
                                }
                            }
                            if !ok {
                                break;
                            }
                        }
                    }
                    0x06 => {
                        // CRYPTO frame
                        let Some(crypto_off) = read_quic_varint(plaintext, &mut fpos) else {
                            break;
                        };
                        let Some(crypto_len) = read_quic_varint(plaintext, &mut fpos) else {
                            break;
                        };
                        if fpos + crypto_len > plaintext.len() {
                            break;
                        }
                        self.insert_chunk(crypto_off, &plaintext[fpos..fpos + crypto_len]);
                        fpos += crypto_len;
                    }
                    _ => break,
                }
            }
            offset = pkt_end;
        }

        if !self.saw_initial {
            return ParseOutcome::NotMatched;
        }
        if !decrypted_this_datagram && self.chunks.is_empty() {
            return ParseOutcome::Incomplete;
        }
        let hs = self.contiguous_prefix();
        if hs.is_empty() {
            return ParseOutcome::Incomplete;
        }
        parse_tls_client_hello_handshake_sni(&hs)
    }
}

pub(crate) async fn sniff_udp_quic_packets(
    reader: &mut dyn RunUdpReader,
    first_packet: UDPPacket,
    max_wait: Duration,
) -> (Option<String>, Vec<UDPPacket>) {
    const MAX_QUIC_SNIFF_PACKETS: usize = 4;

    let mut quic_sniffer = QuicInitialSniffer::default();
    let mut outcome = quic_sniffer.feed_datagram(&first_packet.data);
    let mut buffered = vec![first_packet];

    if outcome == ParseOutcome::Incomplete {
        let deadline = Instant::now() + max_wait;
        while buffered.len() < MAX_QUIC_SNIFF_PACKETS {
            match timeout_at(deadline, reader.read()).await {
                Ok(Ok(next_pkt)) => {
                    let next_outcome = quic_sniffer.feed_datagram(&next_pkt.data);
                    buffered.push(next_pkt);
                    outcome = next_outcome;
                    if outcome != ParseOutcome::Incomplete {
                        break;
                    }
                }
                Ok(Err(_)) | Err(_) => break,
            }
        }
    }

    let domain = match outcome {
        ParseOutcome::Found(host) => Some(host),
        _ => None,
    };
    (domain, buffered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::tcp::TcpRunStream;

    fn build_tls_client_hello(sni: Option<&str>, extra_padding_ext: usize) -> Vec<u8> {
        let mut exts = Vec::new();
        if let Some(host) = sni {
            let host_bytes = host.as_bytes();
            let entry_len = 3 + host_bytes.len();
            let sni_ext_len = 2 + entry_len;
            exts.extend_from_slice(&0x0000u16.to_be_bytes());
            exts.extend_from_slice(&(sni_ext_len as u16).to_be_bytes());
            exts.extend_from_slice(&(entry_len as u16).to_be_bytes());
            exts.push(0x00);
            exts.extend_from_slice(&(host_bytes.len() as u16).to_be_bytes());
            exts.extend_from_slice(host_bytes);
        }
        if extra_padding_ext > 0 {
            exts.extend_from_slice(&0x0015u16.to_be_bytes());
            exts.extend_from_slice(&(extra_padding_ext as u16).to_be_bytes());
            exts.extend(std::iter::repeat_n(0u8, extra_padding_ext));
        }

        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // client_version TLS 1.2
        body.extend_from_slice(&[0x11; 32]); // random
        body.push(0x00); // session_id len
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&[0x13, 0x01]); // TLS_AES_128_GCM_SHA256
        body.push(0x01);
        body.push(0x00); // null compression
        if !exts.is_empty() {
            body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
            body.extend_from_slice(&exts);
        }

        let mut hs = Vec::new();
        hs.push(0x01); // ClientHello
        let bl = body.len();
        hs.push(((bl >> 16) & 0xff) as u8);
        hs.push(((bl >> 8) & 0xff) as u8);
        hs.push((bl & 0xff) as u8);
        hs.extend_from_slice(&body);

        let mut rec = Vec::new();
        rec.push(0x16); // Handshake
        rec.extend_from_slice(&[0x03, 0x01]); // TLS 1.0 record layer
        rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
        rec.extend_from_slice(&hs);
        rec
    }

    #[test]
    fn test_tls_sni_parsing_standard_fragmented_and_missing() {
        let hello = build_tls_client_hello(Some("Shopify-Assets.ShopifyCDN.com."), 0);
        assert_eq!(
            sniff_buffer(&hello),
            ParseOutcome::Found("shopify-assets.shopifycdn.com".to_string())
        );

        // Partial record -> Incomplete
        assert_eq!(sniff_buffer(&hello[..20]), ParseOutcome::Incomplete);

        // Large PQ-style ClientHello (>1600 bytes)
        let large_hello = build_tls_client_hello(Some("llm.rikaaa0928.moe"), 1600);
        assert!(large_hello.len() > 1600);
        assert_eq!(
            sniff_buffer(&large_hello),
            ParseOutcome::Found("llm.rikaaa0928.moe".to_string())
        );

        // ClientHello without SNI
        let no_sni = build_tls_client_hello(None, 32);
        assert_eq!(sniff_buffer(&no_sni), ParseOutcome::CompleteNoHost);

        // IP literal in SNI should be rejected
        let ip_sni = build_tls_client_hello(Some("23.227.39.200"), 0);
        assert_eq!(sniff_buffer(&ip_sni), ParseOutcome::CompleteNoHost);

        // Non-TLS binary protocol
        assert_eq!(
            sniff_buffer(b"SSH-2.0-OpenSSH_9.6\r\n"),
            ParseOutcome::NotMatched
        );
    }

    #[test]
    fn test_http_host_parsing() {
        let req = b"GET /static/fonts/inter/v4/styles.css HTTP/1.1\r\nUser-Agent: curl\r\nHost: Shopify-Assets.ShopifyCDN.com:8080\r\n\r\n";
        assert_eq!(
            sniff_buffer(req),
            ParseOutcome::Found("shopify-assets.shopifycdn.com".to_string())
        );

        let ip_req = b"GET / HTTP/1.1\r\nHost: 192.168.0.71:8080\r\n\r\n";
        assert_eq!(sniff_buffer(ip_req), ParseOutcome::CompleteNoHost);

        let partial = b"GET / HTTP/1.1\r\nHo";
        assert_eq!(sniff_buffer(partial), ParseOutcome::Incomplete);
    }

    #[tokio::test]
    async fn test_sniff_tcp_stream_fragmented_and_server_first_timeout() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Case 1: fragmented TLS ClientHello where SNI is in chunk 1 -> returns early without waiting,
        // and the remaining bytes stay intact in the stream.
        let hello = build_tls_client_hello(Some("shopify-assets.shopifycdn.com"), 1500);
        let split_at = 200;
        let hello_clone = hello.clone();
        let client_task = tokio::spawn(async move {
            let mut c = TcpStream::connect(addr).await.unwrap();
            c.write_all(&hello_clone[..split_at]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(15)).await;
            c.write_all(&hello_clone[split_at..]).await.unwrap();
            let _ = c.shutdown().await;
        });

        let (server_sock, _) = listener.accept().await.unwrap();
        let mut run_stream = TcpRunStream::new(server_sock);
        let (domain, cache) = sniff_tcp_stream(&mut run_stream, Duration::from_millis(200)).await;
        assert_eq!(domain.as_deref(), Some("shopify-assets.shopifycdn.com"));
        let mut full = cache.unwrap();
        let mut tail = [0u8; 2048];
        loop {
            let n = run_stream.read(&mut tail).await.unwrap();
            if n == 0 {
                break;
            }
            full.extend_from_slice(&tail[..n]);
        }
        assert_eq!(full, hello);
        let _ = client_task.await;

        // Case 1b: fragmented TLS ClientHello where 1500B padding comes BEFORE SNI -> waits across 2 writes
        let mut late_exts = Vec::new();
        late_exts.extend_from_slice(&0x0015u16.to_be_bytes());
        late_exts.extend_from_slice(&1500u16.to_be_bytes());
        late_exts.extend(std::iter::repeat_n(0u8, 1500));
        let host_b = b"llm.rikaaa0928.moe";
        late_exts.extend_from_slice(&0x0000u16.to_be_bytes());
        late_exts.extend_from_slice(&((5 + host_b.len()) as u16).to_be_bytes());
        late_exts.extend_from_slice(&((3 + host_b.len()) as u16).to_be_bytes());
        late_exts.push(0x00);
        late_exts.extend_from_slice(&(host_b.len() as u16).to_be_bytes());
        late_exts.extend_from_slice(host_b);
        let mut late_body = vec![0x03, 0x03];
        late_body.extend_from_slice(&[0x11; 32]);
        late_body.push(0x00);
        late_body.extend_from_slice(&2u16.to_be_bytes());
        late_body.extend_from_slice(&[0x13, 0x01, 0x01, 0x00]);
        late_body.extend_from_slice(&(late_exts.len() as u16).to_be_bytes());
        late_body.extend_from_slice(&late_exts);
        let mut late_rec = vec![
            0x16,
            0x03,
            0x01,
            (((4 + late_body.len()) >> 8) & 0xff) as u8,
            ((4 + late_body.len()) & 0xff) as u8,
            0x01,
            0x00,
            ((late_body.len() >> 8) & 0xff) as u8,
            (late_body.len() & 0xff) as u8,
        ];
        late_rec.extend_from_slice(&late_body);
        let late_clone = late_rec.clone();
        let late_client = tokio::spawn(async move {
            let mut c = TcpStream::connect(addr).await.unwrap();
            c.write_all(&late_clone[..300]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(15)).await;
            c.write_all(&late_clone[300..]).await.unwrap();
            c
        });
        let (server_sock_late, _) = listener.accept().await.unwrap();
        let mut run_stream_late = TcpRunStream::new(server_sock_late);
        let (domain_late, cache_late) =
            sniff_tcp_stream(&mut run_stream_late, Duration::from_millis(200)).await;
        assert_eq!(domain_late.as_deref(), Some("llm.rikaaa0928.moe"));
        assert_eq!(cache_late.as_deref(), Some(late_rec.as_slice()));
        drop(late_client.await.unwrap());

        // Case 2: server-first peer sends nothing -> times out promptly with None cache
        let silent_client = tokio::spawn(async move {
            let c = TcpStream::connect(addr).await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            c
        });
        let (server_sock2, _) = listener.accept().await.unwrap();
        let mut run_stream2 = TcpRunStream::new(server_sock2);
        let start = Instant::now();
        let (domain2, cache2) = sniff_tcp_stream(&mut run_stream2, Duration::from_millis(40)).await;
        assert!(start.elapsed() < Duration::from_millis(180));
        assert_eq!(domain2, None);
        assert_eq!(cache2, None);
        drop(silent_client.await.unwrap());
    }

    fn encrypt_quic_initial_packet(
        version: rustls::quic::Version,
        version_wire: u32,
        dcid: &[u8],
        pn: u64,
        crypto_offset: usize,
        crypto_payload: &[u8],
        pad_to: usize,
    ) -> Vec<u8> {
        let suite = rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256
            .tls13()
            .unwrap();
        let keys = rustls::quic::Keys::initial(
            version,
            suite,
            suite.quic.unwrap(),
            dcid,
            rustls::Side::Client,
        );

        let mut plain = Vec::new();
        plain.push(0x06); // CRYPTO
        // Encode varints cleanly for small test offsets/lengths (< 16384)
        for v in [crypto_offset, crypto_payload.len()] {
            if v < 64 {
                plain.push(v as u8);
            } else {
                let enc = (v as u16) | 0x4000;
                plain.extend_from_slice(&enc.to_be_bytes());
            }
        }
        plain.extend_from_slice(crypto_payload);

        // Header prefix up to pn (1-byte pn)
        let first_type_bits = match version {
            rustls::quic::Version::V2 => 0x10,
            _ => 0x00,
        };
        let mut hdr = Vec::new();
        hdr.push(0xc0 | first_type_bits); // pn_len=1 (low 2 bits = 0)
        hdr.extend_from_slice(&version_wire.to_be_bytes());
        hdr.push(dcid.len() as u8);
        hdr.extend_from_slice(dcid);
        hdr.push(0x00); // scid len = 0
        hdr.push(0x00); // token len = 0

        let tag_len = keys.local.packet.tag_len();
        let min_plain = pad_to.saturating_sub(hdr.len() + 2 + 1 + tag_len);
        if plain.len() < min_plain {
            plain.resize(min_plain, 0x00);
        }
        let payload_len = (1 + plain.len() + tag_len) as u16 | 0x4000;
        hdr.extend_from_slice(&payload_len.to_be_bytes());
        let pn_offset = hdr.len();
        hdr.push((pn & 0xff) as u8);

        let tag = keys.local.packet.encrypt_in_place(pn, &hdr, &mut plain).unwrap();
        let mut pkt = hdr;
        pkt.extend_from_slice(&plain);
        pkt.extend_from_slice(tag.as_ref());

        let (first_and_prefix, rest) = pkt.split_at_mut(pn_offset);
        let (pn_area, body) = rest.split_at_mut(1);
        let sample = body[3..3 + 16].to_vec();
        keys.local
            .header
            .encrypt_in_place(&sample, &mut first_and_prefix[0], pn_area)
            .unwrap();
        pkt
    }

    #[test]
    fn test_quic_initial_sni_v1_v2_and_fragmented_pq() {
        // 1. Standard QUIC v1 single-packet ClientHello
        let rec = build_tls_client_hello(Some("Shopify-Assets.ShopifyCDN.com"), 0);
        let hs = &rec[5..];
        let dcid = [0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];
        let pkt_v1 = encrypt_quic_initial_packet(
            rustls::quic::Version::V1,
            0x0000_0001,
            &dcid,
            0,
            0,
            hs,
            1200,
        );
        let mut s1 = QuicInitialSniffer::default();
        assert_eq!(
            s1.feed_datagram(&pkt_v1),
            ParseOutcome::Found("shopify-assets.shopifycdn.com".to_string())
        );

        // 2. QUIC v2 single-packet ClientHello
        let rec2 = build_tls_client_hello(Some("llm.rikaaa0928.moe"), 0);
        let pkt_v2 = encrypt_quic_initial_packet(
            rustls::quic::Version::V2,
            0x6b33_43cf,
            &dcid,
            0,
            0,
            &rec2[5..],
            1200,
        );
        let mut s2 = QuicInitialSniffer::default();
        assert_eq!(
            s2.feed_datagram(&pkt_v2),
            ParseOutcome::Found("llm.rikaaa0928.moe".to_string())
        );

        // 3. Large PQ-style ClientHello where a 1500B padding ext comes BEFORE SNI,
        // split across 2 QUIC Initial packets (delivered out-of-order: pkt 1 then pkt 0!).
        let mut exts = Vec::new();
        exts.extend_from_slice(&0x0033u16.to_be_bytes()); // key_share before SNI
        exts.extend_from_slice(&1400u16.to_be_bytes());
        exts.extend(std::iter::repeat_n(0x42u8, 1400));
        let host_bytes = b"online-store-web.shopifyapps.com";
        let entry_len = 3 + host_bytes.len();
        exts.extend_from_slice(&0x0000u16.to_be_bytes());
        exts.extend_from_slice(&((2 + entry_len) as u16).to_be_bytes());
        exts.extend_from_slice(&(entry_len as u16).to_be_bytes());
        exts.push(0x00);
        exts.extend_from_slice(&(host_bytes.len() as u16).to_be_bytes());
        exts.extend_from_slice(host_bytes);

        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]);
        body.extend_from_slice(&[0x22; 32]);
        body.push(0x00);
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(0x01);
        body.push(0x00);
        body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
        body.extend_from_slice(&exts);

        let mut large_hs = vec![0x01, 0, ((body.len() >> 8) & 0xff) as u8, (body.len() & 0xff) as u8];
        large_hs.extend_from_slice(&body);
        let split_at = 1100;
        let frag0 = encrypt_quic_initial_packet(
            rustls::quic::Version::V1,
            0x0000_0001,
            &dcid,
            0,
            0,
            &large_hs[..split_at],
            1200,
        );
        let frag1 = encrypt_quic_initial_packet(
            rustls::quic::Version::V1,
            0x0000_0001,
            &dcid,
            1,
            split_at,
            &large_hs[split_at..],
            1200,
        );

        let mut s_frag = QuicInitialSniffer::default();
        assert_eq!(s_frag.feed_datagram(&frag1), ParseOutcome::Incomplete);
        assert_eq!(
            s_frag.feed_datagram(&frag0),
            ParseOutcome::Found("online-store-web.shopifyapps.com".to_string())
        );

        // 4. Non-QUIC UDP (e.g. DNS query) -> NotMatched immediately
        let mut s_dns = QuicInitialSniffer::default();
        assert_eq!(
            s_dns.feed_datagram(&[0x12, 0x34, 0x01, 0x00, 0x00, 0x01]),
            ParseOutcome::NotMatched
        );
    }

    #[tokio::test]
    async fn test_real_quinn_initial_packet_sni() {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut tls_cfg = rustls::ClientConfig::builder_with_provider(
            std::sync::Arc::new(rustls::crypto::ring::default_provider()),
        )
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        tls_cfg.alpn_protocols = vec![b"h3".to_vec()];
        let quic_cfg = quinn::crypto::rustls::QuicClientConfig::try_from(tls_cfg).unwrap();
        let client_cfg = quinn::ClientConfig::new(std::sync::Arc::new(quic_cfg));

        let capture_sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target_addr = capture_sock.local_addr().unwrap();

        let mut ep = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        ep.set_default_client_config(client_cfg);
        let _conn_fut = ep.connect(target_addr, "shopify-assets.shopifycdn.com").unwrap();

        let mut buf = [0u8; 2048];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), capture_sock.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let mut sniffer = QuicInitialSniffer::default();
        assert_eq!(
            sniffer.feed_datagram(&buf[..n]),
            ParseOutcome::Found("shopify-assets.shopifycdn.com".to_string())
        );
    }

    struct MockUdpReader {
        rx: tokio::sync::mpsc::Receiver<UDPPacket>,
    }

    #[async_trait::async_trait]
    impl RunUdpReader for MockUdpReader {
        async fn read(&mut self) -> std::io::Result<UDPPacket> {
            self.rx
                .recv()
                .await
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof"))
        }
    }

    #[tokio::test]
    async fn test_sniff_udp_quic_packets_reassembles_and_preserves_order() {
        let mut exts = Vec::new();
        exts.extend_from_slice(&0x0033u16.to_be_bytes());
        exts.extend_from_slice(&1400u16.to_be_bytes());
        exts.extend(std::iter::repeat_n(0x42u8, 1400));
        let host_bytes = b"shopify-assets.shopifycdn.com";
        let entry_len = 3 + host_bytes.len();
        exts.extend_from_slice(&0x0000u16.to_be_bytes());
        exts.extend_from_slice(&((2 + entry_len) as u16).to_be_bytes());
        exts.extend_from_slice(&(entry_len as u16).to_be_bytes());
        exts.push(0x00);
        exts.extend_from_slice(&(host_bytes.len() as u16).to_be_bytes());
        exts.extend_from_slice(host_bytes);

        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]);
        body.extend_from_slice(&[0x33; 32]);
        body.push(0x00);
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(0x01);
        body.push(0x00);
        body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
        body.extend_from_slice(&exts);

        let mut large_hs = vec![
            0x01,
            0,
            ((body.len() >> 8) & 0xff) as u8,
            (body.len() & 0xff) as u8,
        ];
        large_hs.extend_from_slice(&body);

        let dcid = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let split_at = 1100;
        let pkt0 = encrypt_quic_initial_packet(
            rustls::quic::Version::V1,
            0x0000_0001,
            &dcid,
            0,
            0,
            &large_hs[..split_at],
            1200,
        );
        let pkt1 = encrypt_quic_initial_packet(
            rustls::quic::Version::V1,
            0x0000_0001,
            &dcid,
            1,
            split_at,
            &large_hs[split_at..],
            1200,
        );

        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let first = UDPPacket {
            meta: crate::def::UDPMeta {
                src_addr: "192.168.0.48".to_string(),
                src_port: 55000,
                dst_addr: "34.144.250.199".to_string(),
                dst_port: 443,
            },
            data: pkt0.clone(),
        };
        let second = UDPPacket {
            meta: crate::def::UDPMeta {
                src_addr: "192.168.0.48".to_string(),
                src_port: 55000,
                dst_addr: "34.144.250.199".to_string(),
                dst_port: 443,
            },
            data: pkt1.clone(),
        };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let _ = tx.send(second).await;
        });

        let mut reader = MockUdpReader { rx };
        let (domain, buffered) =
            sniff_udp_quic_packets(&mut reader, first, Duration::from_millis(150)).await;
        assert_eq!(domain.as_deref(), Some("shopify-assets.shopifycdn.com"));
        assert_eq!(buffered.len(), 2);
        assert_eq!(buffered[0].data, pkt0);
        assert_eq!(buffered[1].data, pkt1);
    }
}

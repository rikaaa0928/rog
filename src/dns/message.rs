use super::table::canonical;
use hickory_proto::{
    op::{Message, MessageType, OpCode, ResponseCode},
    rr::{DNSClass, RData, RecordType, rdata::svcb::SvcParamValue},
};
use std::{collections::HashSet, net::IpAddr};
pub fn error_reply(query: &Message, code: ResponseCode) -> Vec<u8> {
    let mut reply = Message::new();
    reply
        .set_id(query.id())
        .set_message_type(MessageType::Response)
        .set_op_code(query.op_code())
        .set_recursion_desired(query.recursion_desired())
        .set_recursion_available(true)
        .set_response_code(code);
    if let Some(q) = query.query() {
        reply.add_query(q.clone());
    }
    reply.to_vec().unwrap_or_default()
}
pub fn query_error(query: &Message) -> Option<ResponseCode> {
    if query.op_code() != OpCode::Query {
        return Some(ResponseCode::NotImp);
    }
    if query.queries().len() != 1 {
        return Some(ResponseCode::FormErr);
    }
    let q = &query.queries()[0];
    if q.query_class() != DNSClass::IN
        || matches!(q.query_type(), RecordType::AXFR | RecordType::IXFR)
    {
        return Some(ResponseCode::Refused);
    }
    None
}
pub fn valid_response(query: &Message, response: &Message) -> bool {
    response.message_type() == MessageType::Response
        && response.id() == query.id()
        && response.op_code() == query.op_code()
        && response.queries().len() == 1
        && query.queries().len() == 1
        && {
            let a = &query.queries()[0];
            let b = &response.queries()[0];
            canonical(&a.name().to_ascii()) == canonical(&b.name().to_ascii())
                && a.query_type() == b.query_type()
                && a.query_class() == b.query_class()
        }
}
pub fn cname_chain(query: &Message, response: &Message) -> Vec<(String, u32)> {
    if response.truncated() || response.response_code() != ResponseCode::NoError {
        return vec![];
    }
    let Some(q) = query.query() else {
        return vec![];
    };
    let mut owner = canonical(&q.name().to_ascii());
    let mut ttl = u32::MAX;
    let mut seen = HashSet::new();
    let mut chain = Vec::new();
    for _ in 0..=16 {
        if !seen.insert(owner.clone()) {
            return vec![];
        }
        let aliases: Vec<_> = response
            .answers()
            .iter()
            .filter(|r| {
                r.dns_class() == DNSClass::IN
                    && canonical(&r.name().to_ascii()) == owner
                    && matches!(r.data(), RData::CNAME(_))
            })
            .collect();
        if !aliases.is_empty() {
            if aliases.len() != 1 || seen.len() > 16 {
                return vec![];
            }
            let r = aliases[0];
            ttl = ttl.min(r.ttl());
            if let RData::CNAME(name) = r.data() {
                owner = canonical(&name.0.to_ascii());
                chain.push((owner.clone(), ttl));
            }
            continue;
        }
        return chain;
    }
    vec![]
}

pub fn addresses(query: &Message, response: &Message) -> Vec<(IpAddr, u32)> {
    if response.truncated() || response.response_code() != ResponseCode::NoError {
        return vec![];
    }
    let Some(q) = query.query() else {
        return vec![];
    };
    let mut owner = canonical(&q.name().to_ascii());
    let mut ttl = u32::MAX;
    let mut seen = HashSet::new();
    for _ in 0..=16 {
        if !seen.insert(owner.clone()) {
            return vec![];
        }
        let aliases: Vec<_> = response
            .answers()
            .iter()
            .filter(|r| {
                r.dns_class() == DNSClass::IN
                    && canonical(&r.name().to_ascii()) == owner
                    && matches!(r.data(), RData::CNAME(_))
            })
            .collect();
        if !aliases.is_empty() {
            if aliases.len() != 1 || seen.len() > 16 {
                return vec![];
            }
            let r = aliases[0];
            ttl = ttl.min(r.ttl());
            if let RData::CNAME(name) = r.data() {
                owner = canonical(&name.0.to_ascii());
            }
            continue;
        }
        let qtype = q.query_type();
        let mut out = Vec::new();
        let mut svcb_targets = HashSet::new();
        for r in response.answers() {
            if r.dns_class() != DNSClass::IN || canonical(&r.name().to_ascii()) != owner {
                continue;
            }
            let rec_ttl = ttl.min(r.ttl());
            match r.data() {
                RData::A(a) if qtype == RecordType::A || qtype == RecordType::ANY => {
                    out.push((IpAddr::V4(a.0), rec_ttl));
                }
                RData::AAAA(a) if qtype == RecordType::AAAA || qtype == RecordType::ANY => {
                    out.push((IpAddr::V6(a.0), rec_ttl));
                }
                RData::HTTPS(https)
                    if qtype == RecordType::HTTPS || qtype == RecordType::ANY =>
                {
                    let svcb = &https.0;
                    let target = if svcb.target_name().is_root() {
                        owner.clone()
                    } else {
                        canonical(&svcb.target_name().to_ascii())
                    };
                    svcb_targets.insert((target, rec_ttl));
                    for (_, param) in svcb.svc_params() {
                        match param {
                            SvcParamValue::Ipv4Hint(hint) => {
                                for a in &hint.0 {
                                    out.push((IpAddr::V4(a.0), rec_ttl));
                                }
                            }
                            SvcParamValue::Ipv6Hint(hint) => {
                                for aaaa in &hint.0 {
                                    out.push((IpAddr::V6(aaaa.0), rec_ttl));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                RData::SVCB(svcb)
                    if qtype == RecordType::SVCB || qtype == RecordType::ANY =>
                {
                    let target = if svcb.target_name().is_root() {
                        owner.clone()
                    } else {
                        canonical(&svcb.target_name().to_ascii())
                    };
                    svcb_targets.insert((target, rec_ttl));
                    for (_, param) in svcb.svc_params() {
                        match param {
                            SvcParamValue::Ipv4Hint(hint) => {
                                for a in &hint.0 {
                                    out.push((IpAddr::V4(a.0), rec_ttl));
                                }
                            }
                            SvcParamValue::Ipv6Hint(hint) => {
                                for aaaa in &hint.0 {
                                    out.push((IpAddr::V6(aaaa.0), rec_ttl));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if !svcb_targets.is_empty() {
            for r in response.answers().iter().chain(response.additionals().iter()) {
                if r.dns_class() != DNSClass::IN {
                    continue;
                }
                let r_name = canonical(&r.name().to_ascii());
                let Some((_, base_ttl)) = svcb_targets.iter().find(|(t, _)| *t == r_name) else {
                    continue;
                };
                let rec_ttl = (*base_ttl).min(r.ttl());
                match r.data() {
                    RData::A(a) => {
                        let ip = IpAddr::V4(a.0);
                        if !out.iter().any(|(existing, _)| *existing == ip) {
                            out.push((ip, rec_ttl));
                        }
                    }
                    RData::AAAA(a) => {
                        let ip = IpAddr::V6(a.0);
                        if !out.iter().any(|(existing, _)| *existing == ip) {
                            out.push((ip, rec_ttl));
                        }
                    }
                    _ => {}
                }
            }
        }
        return out;
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::{
        op::Query,
        rr::{
            Name, Record,
            rdata::{A, CNAME},
        },
    };
    pub(super) fn query() -> Message {
        let mut q = Message::new();
        q.set_id(42).add_query(Query::query(
            Name::from_ascii("A.Example.").unwrap(),
            RecordType::A,
        ));
        q
    }
    #[test]
    fn validation_chain_and_ttl() {
        let q = query();
        let mut r = Message::new();
        r.set_id(42)
            .set_message_type(MessageType::Response)
            .add_query(q.queries()[0].clone());
        r.add_answer(Record::from_rdata(
            Name::from_ascii("a.example").unwrap(),
            10,
            RData::CNAME(CNAME(Name::from_ascii("b.example").unwrap())),
        ));
        r.add_answer(Record::from_rdata(
            Name::from_ascii("b.example").unwrap(),
            30,
            RData::A(A("1.1.1.1".parse().unwrap())),
        ));
        r.add_additional(Record::from_rdata(
            Name::from_ascii("unrelated").unwrap(),
            90,
            RData::A(A("8.8.8.8".parse().unwrap())),
        ));
        assert!(valid_response(&q, &r));
        assert_eq!(addresses(&q, &r), vec![("1.1.1.1".parse().unwrap(), 10)]);
        assert_eq!(cname_chain(&q, &r), vec![("b.example".to_string(), 10)]);
        r.set_id(43);
        assert!(!valid_response(&q, &r));
        r.set_id(42).set_truncated(true);
        assert!(addresses(&q, &r).is_empty());
        r.set_truncated(false)
            .set_response_code(ResponseCode::NXDomain);
        assert!(addresses(&q, &r).is_empty());
    }
    #[test]
    fn errors_and_cycle() {
        let q = query();
        let e = Message::from_vec(&error_reply(&q, ResponseCode::ServFail)).unwrap();
        assert!(valid_response(&q, &e));
        assert_eq!(e.response_code(), ResponseCode::ServFail);
        let mut r = e;
        r.set_response_code(ResponseCode::NoError);
        r.add_answer(Record::from_rdata(
            q.queries()[0].name().clone(),
            30,
            RData::CNAME(CNAME(q.queries()[0].name().clone())),
        ));
        assert!(addresses(&q, &r).is_empty());
        let mut bad = q;
        bad.add_query(bad.queries()[0].clone());
        assert_eq!(query_error(&bad), Some(ResponseCode::FormErr));
    }
    #[test]
    fn https_svcb_hints_and_cname_only() {
        use hickory_proto::rr::rdata::{
            AAAA, HTTPS,
            svcb::{IpHint, SVCB, SvcParamKey, SvcParamValue},
        };
        let mut q = Message::new();
        q.set_id(7).add_query(Query::query(
            Name::from_ascii("chat.example.com.").unwrap(),
            RecordType::HTTPS,
        ));
        let mut r = Message::new();
        r.set_id(7)
            .set_message_type(MessageType::Response)
            .add_query(q.queries()[0].clone());
        r.add_answer(Record::from_rdata(
            Name::from_ascii("chat.example.com.").unwrap(),
            300,
            RData::CNAME(CNAME(Name::from_ascii("chat.cdn.example.net.").unwrap())),
        ));
        let svcb = SVCB::new(
            1,
            Name::root(),
            vec![
                (
                    SvcParamKey::Ipv4Hint,
                    SvcParamValue::Ipv4Hint(IpHint(vec![A("104.21.53.236".parse().unwrap())])),
                ),
                (
                    SvcParamKey::Ipv6Hint,
                    SvcParamValue::Ipv6Hint(IpHint(vec![AAAA(
                        "2606:4700:3034::6815:35ec".parse().unwrap(),
                    )])),
                ),
            ],
        );
        r.add_answer(Record::from_rdata(
            Name::from_ascii("chat.cdn.example.net.").unwrap(),
            120,
            RData::HTTPS(HTTPS(svcb)),
        ));
        assert_eq!(
            cname_chain(&q, &r),
            vec![("chat.cdn.example.net".to_string(), 300)]
        );
        assert_eq!(
            addresses(&q, &r),
            vec![
                ("104.21.53.236".parse().unwrap(), 120),
                ("2606:4700:3034::6815:35ec".parse().unwrap(), 120),
            ]
        );
    }
}

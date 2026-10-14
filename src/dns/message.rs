use super::table::canonical;
use hickory_proto::{
    op::{Message, MessageType, OpCode, ResponseCode},
    rr::{DNSClass, RData, RecordType},
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
        return response
            .answers()
            .iter()
            .filter_map(|r| {
                if r.dns_class() != DNSClass::IN || canonical(&r.name().to_ascii()) != owner {
                    return None;
                }
                let ip = match r.data() {
                    RData::A(a)
                        if q.query_type() == RecordType::A || q.query_type() == RecordType::ANY =>
                    {
                        IpAddr::V4(a.0)
                    }
                    RData::AAAA(a)
                        if q.query_type() == RecordType::AAAA
                            || q.query_type() == RecordType::ANY =>
                    {
                        IpAddr::V6(a.0)
                    }
                    _ => return None,
                };
                Some((ip, ttl.min(r.ttl())))
            })
            .collect();
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
}

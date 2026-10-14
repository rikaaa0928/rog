use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

pub fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        _ => ip,
    }
}
pub fn canonical(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}
#[derive(Clone)]
struct Record {
    domain: String,
    expires: Instant,
    observed: Instant,
}
type Key = (String, IpAddr, IpAddr);
pub struct IpDomainTable {
    entries: Mutex<HashMap<Key, Vec<Record>>>,
    capacity: usize,
    per_ip: usize,
    max_ttl: u64,
}
#[derive(Clone, serde::Serialize)]
pub struct MappingSnapshot {
    pub router: String,
    pub client_ip: IpAddr,
    pub target_ip: IpAddr,
    pub domain: String,
    pub ttl_remaining_ms: u64,
    pub last_observed_ago_ms: u64,
}
#[derive(serde::Serialize)]
pub struct TableSnapshot {
    pub enabled: bool,
    pub key_count: usize,
    pub record_count: usize,
    pub capacity: usize,
    pub max_domains_per_ip: usize,
    pub max_ttl_secs: u64,
    pub entries: Vec<MappingSnapshot>,
}
impl IpDomainTable {
    pub fn new(capacity: usize, per_ip: usize, max_ttl: u64) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity,
            per_ip,
            max_ttl,
        }
    }
    pub fn record(
        &self,
        router: &str,
        client: IpAddr,
        target: IpAddr,
        domain: &str,
        ttl: u32,
        received: Instant,
    ) {
        let now = Instant::now();
        let Some(expires) =
            received.checked_add(Duration::from_secs((ttl as u64).min(self.max_ttl)))
        else {
            return;
        };
        if expires <= now || self.capacity == 0 || self.per_ip == 0 {
            return;
        }
        let key = (router.to_owned(), normalize(client), normalize(target));
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|_, records| {
            records.retain(|r| r.expires > now);
            !records.is_empty()
        });
        if !entries.contains_key(&key) && entries.len() >= self.capacity {
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, rs)| rs.iter().map(|r| r.observed).max())
                .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest);
            }
        }
        let records = entries.entry(key).or_default();
        let domain = canonical(domain);
        if let Some(record) = records.iter_mut().find(|r| r.domain == domain) {
            record.expires = record.expires.max(expires);
            record.observed = now;
            return;
        }
        if records.len() >= self.per_ip {
            let oldest = records
                .iter()
                .enumerate()
                .min_by_key(|(_, r)| r.observed)
                .unwrap()
                .0;
            records.remove(oldest);
        }
        records.push(Record {
            domain,
            expires,
            observed: now,
        });
    }
    pub fn get_domains(&self, router: &str, client: IpAddr, target: IpAddr) -> Vec<String> {
        let now = Instant::now();
        let client = normalize(client);
        let target = normalize(target);
        let mut entries = self.entries.lock().unwrap();
        if let Some(records) = entries.get_mut(&(router.into(), client, target)) {
            records.retain(|r| r.expires > now);
            if !records.is_empty() {
                let mut domains: Vec<_> = records.iter().map(|r| r.domain.clone()).collect();
                domains.sort();
                return domains;
            }
        }
        // Dual-stack / IPv6 privacy address fallback:
        // A client may query DNS over IPv4 and connect to AAAA over IPv6 (or vice versa),
        // or query DNS from a SLAAC IPv6 address and connect from a temporary IPv6 address.
        let mut domains = Vec::new();
        for ((r, c, t), records) in entries.iter_mut() {
            if r == router
                && *t == target
                && (client.is_ipv6() || target.is_ipv6() || c.is_ipv6() != client.is_ipv6())
            {
                records.retain(|rec| rec.expires > now);
                for rec in records.iter() {
                    if !domains.contains(&rec.domain) {
                        domains.push(rec.domain.clone());
                    }
                }
            }
        }
        domains.sort();
        domains
    }
    /// Read-only snapshot: excludes expired records without changing routing state.
    pub fn snapshot(&self) -> TableSnapshot {
        let now = Instant::now();
        let (mut records, key_count) = {
            let entries = self.entries.lock().unwrap();
            let mut records = vec![];
            let mut key_count = 0;
            for ((router, client_ip, target_ip), domains) in entries.iter() {
                let before = records.len();
                for record in domains.iter().filter(|r| r.expires > now) {
                    records.push(MappingSnapshot {
                        router: router.clone(),
                        client_ip: *client_ip,
                        target_ip: *target_ip,
                        domain: record.domain.clone(),
                        ttl_remaining_ms: record.expires.saturating_duration_since(now).as_millis()
                            as u64,
                        last_observed_ago_ms: now
                            .saturating_duration_since(record.observed)
                            .as_millis() as u64,
                    });
                }
                if records.len() > before {
                    key_count += 1;
                }
            }
            (records, key_count)
        };
        records.sort_by(|a, b| {
            (&a.router, a.client_ip, a.target_ip, &a.domain).cmp(&(
                &b.router,
                b.client_ip,
                b.target_ip,
                &b.domain,
            ))
        });
        TableSnapshot {
            enabled: true,
            key_count,
            record_count: records.len(),
            capacity: self.capacity,
            max_domains_per_ip: self.per_ip,
            max_ttl_secs: self.max_ttl,
            entries: records,
        }
    }
    pub fn prune_expired(&self) {
        let now = Instant::now();
        self.entries.lock().unwrap().retain(|_, rs| {
            rs.retain(|r| r.expires > now);
            !rs.is_empty()
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_excludes_expired_records_without_mutating_table() {
        let t = IpDomainTable::new(10, 3, 120);
        let client = "192.168.1.2".parse().unwrap();
        let target = "1.1.1.1".parse().unwrap();
        for name in ["z.example", "A.EXAMPLE."] {
            t.record("main", client, target, name, 60, Instant::now());
        }
        {
            let mut entries = t.entries.lock().unwrap();
            entries
                .get_mut(&("main".into(), client, target))
                .unwrap()
                .push(Record {
                    domain: "expired.example".into(),
                    expires: Instant::now() - Duration::from_secs(1),
                    observed: Instant::now() - Duration::from_secs(10),
                });
        }
        let snapshot = t.snapshot();
        assert_eq!(snapshot.key_count, 1);
        assert_eq!(snapshot.record_count, 2);
        assert_eq!(snapshot.capacity, 10);
        assert_eq!(
            snapshot
                .entries
                .iter()
                .map(|r| r.domain.as_str())
                .collect::<Vec<_>>(),
            vec!["a.example", "z.example"]
        );
        assert!(
            snapshot
                .entries
                .iter()
                .all(|r| r.ttl_remaining_ms <= 60_000)
        );
        assert_eq!(t.entries.lock().unwrap().values().next().unwrap().len(), 3);
    }
    #[test]
    fn isolation_ttl_capacity() {
        let t = IpDomainTable::new(1, 2, 100);
        let c = "192.168.1.2".parse().unwrap();
        let ip = "1.1.1.1".parse().unwrap();
        for name in ["A.COM.", "b.com", "c.com"] {
            t.record("r", c, ip, name, 60, Instant::now());
        }
        assert_eq!(t.get_domains("r", c, ip), vec!["b.com", "c.com"]);
        assert!(t.get_domains("other", c, ip).is_empty());
        assert!(
            t.get_domains("r", "192.168.1.3".parse().unwrap(), ip)
                .is_empty()
        );
        t.record("r", c, ip, "zero", 0, Instant::now());
        assert_eq!(t.get_domains("r", c, ip).len(), 2);
        t.record(
            "r",
            c,
            "8.8.8.8".parse().unwrap(),
            "expired",
            1,
            Instant::now() - Duration::from_secs(2),
        );
        assert_eq!(t.get_domains("r", c, ip).len(), 2);
        t.record(
            "r",
            c,
            "8.8.8.8".parse().unwrap(),
            "new",
            20,
            Instant::now(),
        );
        assert!(t.get_domains("r", c, ip).is_empty());
        // Dual-stack fallback: DNS queried over IPv4, connection made to AAAA over IPv6
        let v6_target: IpAddr = "2404:6800:400a:1007::71".parse().unwrap();
        let v6_client: IpAddr = "240e:305:1890:3000::abcd".parse().unwrap();
        t.record("r", c, v6_target, "google.com", 60, Instant::now());
        assert_eq!(
            t.get_domains("r", v6_client, v6_target),
            vec!["google.com"]
        );
    }
}

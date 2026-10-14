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
        let mut entries = self.entries.lock().unwrap();
        let Some(records) = entries.get_mut(&(router.into(), normalize(client), normalize(target)))
        else {
            return vec![];
        };
        records.retain(|r| r.expires > Instant::now());
        let mut domains: Vec<_> = records.iter().map(|r| r.domain.clone()).collect();
        domains.sort();
        domains
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
    }
}

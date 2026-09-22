use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::router::consts;
use crate::router::matcher::{Matcher, get_matcher_factory_fn};

pub struct DynamicRemoteMatcher {
    name: String,
    url: String,
    inner: Arc<RwLock<Option<Box<dyn Matcher>>>>,
    initialized: Arc<AtomicBool>,
}

impl DynamicRemoteMatcher {
    pub fn new(name: String, url: String, format: String, interval: Duration) -> Self {
        let inner = Arc::new(RwLock::new(None));
        let initialized = Arc::new(AtomicBool::new(false));

        let inner_clone = inner.clone();
        let init_clone = initialized.clone();
        let name_clone = name.clone();
        let url_clone = url.clone();
        let format_clone = format.clone();

        let task = async move {
            Self::fetch_and_update(
                &name_clone,
                &url_clone,
                &format_clone,
                interval,
                &inner_clone,
                &init_clone,
            )
            .await;

            let mut timer = tokio::time::interval(interval);
            timer.tick().await; // The first tick completes immediately; skip it since we just did initial fetch

            loop {
                timer.tick().await;
                Self::fetch_and_update(
                    &name_clone,
                    &url_clone,
                    &format_clone,
                    interval,
                    &inner_clone,
                    &init_clone,
                )
                .await;
            }
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(task);
        } else {
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(task);
                }
            });
        }

        Self {
            name,
            url,
            inner,
            initialized,
        }
    }

    async fn fetch_source(url: &str) -> Result<String, String> {
        if let Some(path) = url.strip_prefix("file://") {
            std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read file '{}': {}", path, e))
        } else {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| format!("failed to build client: {}", e))?;
            let resp = client.get(url).send().await.map_err(|e| format!("{}", e))?;
            if !resp.status().is_success() {
                return Err(format!("HTTP status {}", resp.status()));
            }
            resp.text().await.map_err(|e| format!("{}", e))
        }
    }

    async fn fetch_source_with_retry(url: &str, interval: Duration) -> Result<String, String> {
        let mut last_err = String::new();
        for attempt in 0..=3 {
            if attempt > 0 {
                let base_delay = std::cmp::max(
                    Duration::from_millis(50),
                    std::cmp::min(Duration::from_secs(1), interval / 8),
                );
                let backoff = base_delay * (1 << (attempt - 1));
                tokio::time::sleep(backoff).await;
            }
            match Self::fetch_source(url).await {
                Ok(body) => return Ok(body),
                Err(e) => {
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    async fn fetch_and_update(
        name: &str,
        url: &str,
        format: &str,
        interval: Duration,
        inner: &Arc<RwLock<Option<Box<dyn Matcher>>>>,
        initialized: &Arc<AtomicBool>,
    ) {
        match Self::fetch_source_with_retry(url, interval).await {
            Ok(body) => {
                let is_suffix = format == consts::FORMAT_DOMAIN_SUFFIX
                    || format == consts::FORMAT_DOMAIN_SUFFIX_ALT1
                    || format == consts::FORMAT_DOMAIN_SUFFIX_ALT2;

                let lines: Vec<String> = body
                    .lines()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty() && !s.starts_with('#'))
                    .map(|s| {
                        if is_suffix {
                            let mut val = s;
                            if val.starts_with("*.") {
                                val = &val[2..];
                            } else if val.starts_with('.') {
                                val = &val[1..];
                            }
                            val.to_string()
                        } else {
                            s.to_string()
                        }
                    })
                    .filter(|s| !s.is_empty())
                    .collect();

                let count = lines.len();
                if let Some(factory) = get_matcher_factory_fn(format) {
                    let matcher = factory(lines, Vec::new());
                    if let Ok(mut guard) = inner.write() {
                        *guard = Some(matcher);
                    }
                    initialized.store(true, Ordering::Release);
                    log::info!(
                        "Remote route data '{}' from '{}' loaded/refreshed successfully with {} rules",
                        name,
                        url,
                        count
                    );
                } else {
                    log::warn!("Unsupported route data format '{}' for '{}'", format, name);
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to fetch remote route data '{}' from '{}' after retries: {}, keeping existing data",
                    name,
                    url,
                    e
                );
            }
        }
    }
}

impl Matcher for DynamicRemoteMatcher {
    fn match_host(&self, host: &str) -> bool {
        if !self.initialized.load(Ordering::Acquire) {
            log::warn!(
                "Remote route data '{}' from '{}' loading is not complete yet, treating as empty list",
                self.name,
                self.url
            );
            return false;
        }
        if let Ok(guard) = self.inner.read() {
            guard.as_ref().map(|m| m.match_host(host)).unwrap_or(false)
        } else {
            false
        }
    }
}

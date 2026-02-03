use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::select;
use tokio::sync::Notify;
use log::{debug, info};

pub struct BlockManager {
    block_limit: u64,
    taken_blocks: AtomicU64,
    take_count: AtomicU64,
    notify: Notify,
}

impl BlockManager {
    pub fn new(block_limit: u64) -> BlockManager {
        BlockManager {
            block_limit,
            taken_blocks: Default::default(),
            take_count: Default::default(),
            notify: Notify::new(),
        }
    }

    pub fn can_take(&self) -> bool {
        self.taken_blocks.load(Ordering::Relaxed) < self.block_limit
    }
    pub fn take(&self) {
        self.taken_blocks.fetch_add(1, Ordering::Relaxed);
        let count = self.take_count.fetch_add(1, Ordering::Relaxed);
        let taken = self.taken_blocks.load(Ordering::Relaxed);
        if count % 1000 == 0 {
            info!("BlockManager: take count {}, taken_blocks/block_limit: {}/{}", count, taken, self.block_limit);
        } else if count % 100 == 0 {
            debug!("BlockManager: take count {}, taken_blocks/block_limit: {}/{}", count, taken, self.block_limit);
        }
    }
    pub fn release(&self) {
        self.taken_blocks.fetch_sub(1, Ordering::Relaxed);
        self.notify.notify_one();
    }
    pub async fn wait(&self) {
        self.notify.notified().await;
    }
}

use bytes::Bytes;

pub struct DataBlock {
    block_manager: Arc<BlockManager>,
    data: Mutex<VecDeque<Bytes>>,
    provide_notify: Notify,
    consume_notify: Notify,
}

impl DataBlock {
    pub fn new(block_manager: Arc<BlockManager>) -> Self {
        DataBlock {
            block_manager,
            data: Mutex::new(VecDeque::new()),
            provide_notify: Notify::new(),
            consume_notify: Notify::new(),
        }
    }

    pub async fn provide(&self, data: Bytes) {
        if self.inner_provide(data.clone()) {
            return;
        }
        loop {
            select! {
                _=self.consume_notify.notified() => {

                },
                _=self.block_manager.wait() => {

                },
            }
            if self.inner_provide(data.clone()) {
                return;
            }
        }
    }

    fn inner_provide(&self, data: Bytes) -> bool {
        // 允许一定量的超出限制, 为空必定写入，防止阻塞流量
        if self.data.lock().unwrap().is_empty() {
            self.block_manager.take();
            self.data.lock().unwrap().push_back(data);
            self.provide_notify.notify_one();
            return true;
        }
        // 允许一定量的超出限制
        if self.block_manager.can_take() {
            self.block_manager.take();
            self.data.lock().unwrap().push_back(data);
            self.provide_notify.notify_one();
            return true;
        }
        false
    }

    pub async fn consume(&self) -> Bytes {
        if self.data.lock().unwrap().is_empty() {
            loop {
                self.provide_notify.notified().await;
                if !self.data.lock().unwrap().is_empty() {
                    let data = self.data.lock().unwrap().pop_front().unwrap();
                    self.block_manager.release();
                    self.consume_notify.notify_one();
                    return data;
                }
            }
        } else {
            let data = self.data.lock().unwrap().pop_front().unwrap();
            self.block_manager.release();
            self.consume_notify.notify_one();
            return data;
        }
    }
}

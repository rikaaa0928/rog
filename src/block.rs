use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct BlockManager {
    block_limit: u64,
    taken_blocks: AtomicU64,
}

impl BlockManager {
    pub fn new(block_limit: u64) -> BlockManager {
        BlockManager{
            block_limit,
            taken_blocks: Default::default(),
        }
    }
    
    pub fn can_take(&self) -> bool {
        self.block_limit <= self.taken_blocks.load(Ordering::Relaxed)
    }
    pub fn take(&self)  {
        self.taken_blocks.fetch_add(1, Ordering::Relaxed);
    }
    pub fn release(&self) {
        self.taken_blocks.fetch_sub(1, Ordering::Relaxed);
    }
}
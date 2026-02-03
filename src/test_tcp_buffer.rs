
#[cfg(test)]
mod tests {
    use crate::block::{BlockManager, DataBlock};
    use bytes::Bytes;
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_block_flow() {
        // Limit 2 blocks.
        let limit = 2; // Total items held in queue before blocking: 2?
        // Logic:
        // Provide 1: empty -> take -> push. (Taken=1)
        // Provide 2: taken(1) < 2 -> take -> push. (Taken=2)
        // Provide 3: taken(2) < 2 -> False. Block.

        let bm = Arc::new(BlockManager::new(limit));
        let db = Arc::new(DataBlock::new(bm.clone()));

        // Push 1 item.
        let res = db_provide_helper(db.clone(), Bytes::from_static(&[1])).await;
        assert!(res, "First push should succeed");
        assert_eq!(bm.can_take(), true, "limit 2, taken 1. 1 < 2 should be true");

        // Push 2nd item.
        let res = db_provide_helper(db.clone(), Bytes::from_static(&[2])).await;
        assert!(res, "Second push should succeed");
        assert_eq!(bm.can_take(), false, "limit 2, taken 2. 2 < 2 should be false");

        // Push 3rd item in background. Should block.
        let db_clone = db.clone();
        let h = tokio::spawn(async move {
            db_clone.provide(Bytes::from_static(&[3])).await;
        });

        // Wait a bit to ensure it blocks
        sleep(Duration::from_millis(100)).await;
        assert!(!h.is_finished(), "Third push should block");

        // Consume 1 item.
        let val = db.consume().await;
        assert_eq!(val, Bytes::from_static(&[1]));
        
        // Wait for background push to finish
        sleep(Duration::from_millis(50)).await;
        assert!(h.is_finished(), "Third push should unblock after consume");
        
        // Verify final state
        // Queue has [2, 3]. Taken = 2.
        assert_eq!(bm.can_take(), false, "Taken should be 2");
        
        let val2 = db.consume().await;
        assert_eq!(val2, Bytes::from_static(&[2]));
        assert_eq!(bm.can_take(), true, "Taken should be 1");
        
        let val3 = db.consume().await;
        assert_eq!(val3, Bytes::from_static(&[3]));
        assert_eq!(bm.can_take(), true, "Taken should be 0");
    }

    async fn db_provide_helper(db: Arc<DataBlock>, data: Bytes) -> bool {
        // We use select with timeout to check if it blocks immediately, 
        // but provide returns void (async).
        // If we want to check return immediate, likely need spawn.
        // Or just await it.
        db.provide(data).await;
        true
    }
}

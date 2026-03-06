// SPDX-License-Identifier: Apache-2.0
//! Blockchain types and implementations

use std::collections::VecDeque;

use tracing::error;

use super::{Address, Block};

/// Practical details of the blockchain implementation, such as storing proposed blocks
/// and keeping a list of validators.
///
/// Note that in the dafny spec, the blockchain is just a sequence of blocks,
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Blockchain {
    proposed_block: Option<Block>,
    blocks: VecDeque<Block>,
}

impl Blockchain {
    const MAX_BLOCKS: usize = 32;

    pub fn new(blocks: VecDeque<Block>) -> Self {
        assert!(
            !blocks.is_empty(),
            "Blockchain must have at least one block (the genesis block)"
        );
        Self {
            proposed_block: None,
            blocks,
        }
    }

    pub fn set_proposed_block(&mut self, block: Option<Block>) {
        self.proposed_block = block;
    }

    pub(crate) fn proposed_block(&self) -> Option<&Block> {
        self.proposed_block.as_ref()
    }

    pub fn take_proposed_block(&mut self) -> Option<Block> {
        self.proposed_block.take()
    }

    pub fn blocks(&self) -> &VecDeque<Block> {
        &self.blocks
    }

    pub fn height(&self) -> u64 {
        self.head().header.height + 1
    }

    pub fn head(&self) -> &Block {
        self.blocks().back().expect("Blockchain is empty")
    }

    pub fn set_head(&mut self, block: Block) {
        if block.header.validators.is_empty() {
            error!("Block must have validators set");
        }
        self.blocks.push_back(block);
    }

    pub fn validators(&self) -> &Vec<Address> {
        &self.head().header.validators
    }

    // We only need the last two blocks for QBFT operation.
    pub(crate) fn prune(&mut self) {
        if self.blocks.len() > Self::MAX_BLOCKS {
            let to_remove = self.blocks.len() - Self::MAX_BLOCKS;
            for _ in 0..to_remove {
                self.blocks.pop_front();
            }
        }
    }

    pub fn timestamp_secs(&self) -> u64 {
        self.head().header.timestamp
    }

    /// Returns a fractional timestamp in milliseconds for the head block.
    ///
    /// Estimates sub-second intervals based on the number of preceding blocks with the same
    /// timestamp and the provided block interval in milliseconds. Used for sub-second block
    /// intervals, where multiple blocks may share the same timestamp in seconds.
    pub fn estimate_timestamp_ms(&self, block_interval_ms: u64) -> u128 {
        let mut blocks_with_this_ts = 0_usize;
        let ts = self.timestamp_secs();
        for block in self.blocks.iter().rev().skip(1) {
            if block.header.timestamp == ts {
                blocks_with_this_ts += 1;
            } else {
                break;
            }
        }

        (ts as u128) * 1000 + (blocks_with_this_ts as u128) * block_interval_ms as u128
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Block, BlockHeader};
    use alloy_primitives::Bytes;
    use std::collections::VecDeque;

    fn create_test_block(height: u64, timestamp: u64, validators: Vec<Address>) -> Block {
        let header = BlockHeader {
            proposer: validators.first().copied().unwrap_or_default(),
            round_number: 0,
            commit_seals: vec![],
            height,
            timestamp,
            validators,
            digest: Default::default(),
        };
        Block::new(
            header,
            Bytes::from(format!("test-block-{height}-{timestamp}").into_bytes()),
        )
    }

    #[test]
    fn test_estimate_timestamp_ms_single_block() {
        // Test with a single block (genesis)
        let validators = vec![Address::from([1u8; 20])];
        let genesis = create_test_block(0, 1000, validators); // timestamp = 1000 seconds
        let blockchain = Blockchain::new(VecDeque::from([genesis]));

        // With 100ms block interval
        let result = blockchain.estimate_timestamp_ms(100);
        // Should be: 1000 seconds * 1000 ms/sec + 0 * 100ms = 1,000,000 ms
        assert_eq!(result, 1_000_000);

        // With 1000ms block interval
        let result = blockchain.estimate_timestamp_ms(1000);
        assert_eq!(result, 1_000_000);
    }

    #[test]
    fn test_estimate_timestamp_ms_multiple_blocks_different_timestamps() {
        // Test with blocks having different timestamps
        let validators = vec![Address::from([1u8; 20])];
        let block1 = create_test_block(0, 1000, validators.clone()); // 1000 seconds
        let block2 = create_test_block(1, 1001, validators.clone()); // 1001 seconds
        let block3 = create_test_block(2, 1002, validators.clone()); // 1002 seconds

        let blocks = VecDeque::from([block1, block2, block3]);
        let blockchain = Blockchain::new(blocks);

        // Since the head block (block3) has timestamp 1002 and no previous blocks
        // share this timestamp, number_of_blocks_with_this_timestamp = 0
        let result = blockchain.estimate_timestamp_ms(100);
        // Should be: 1002 * 1000 + 0 * 100 = 1,002,000 ms
        assert_eq!(result, 1_002_000);
    }

    #[test]
    fn test_estimate_timestamp_ms_multiple_blocks_same_timestamp() {
        // Test with multiple blocks having the same timestamp (sub-second intervals)
        let validators = vec![Address::from([1u8; 20])];
        let block1 = create_test_block(0, 1000, validators.clone()); // 1000 seconds
        let block2 = create_test_block(1, 1000, validators.clone()); // 1000 seconds (same)
        let block3 = create_test_block(2, 1000, validators.clone()); // 1000 seconds (same)
        let block4 = create_test_block(3, 1000, validators.clone()); // 1000 seconds (same)

        let blocks = VecDeque::from([block1, block2, block3, block4]);
        let blockchain = Blockchain::new(blocks);

        // The head block (block4) has timestamp 1000
        // Looking backward: block3, block2, block1 all have timestamp 1000
        // So number_of_blocks_with_this_timestamp = 3
        // 100ms intervals
        // Should be: 1000 * 1000 + 3 * 100 = 1,000,000 + 300 = 1,000,300 ms
        let result = blockchain.estimate_timestamp_ms(100);
        assert_eq!(result, 1_000_300);

        // Test with different block interval
        // 250ms intervals
        // Should be: 1000 * 1000 + 3 * 250 = 1,000,000 + 750 = 1,000,750 ms
        let result = blockchain.estimate_timestamp_ms(250);
        assert_eq!(result, 1_000_750);
    }

    #[test]
    fn test_estimate_timestamp_ms_mixed_timestamps() {
        // Test with a mix: some blocks share timestamps, others don't
        let validators = vec![Address::from([1u8; 20])];
        // different timestamp
        let block1 = create_test_block(0, 999, validators.clone());
        // start of same timestamp group
        let block2 = create_test_block(1, 1000, validators.clone());
        // same as block2
        let block3 = create_test_block(2, 1000, validators.clone());
        // same as block2, block3 (head)
        let block4 = create_test_block(3, 1000, validators.clone());

        let blocks = VecDeque::from([block1, block2, block3, block4]);
        let blockchain = Blockchain::new(blocks);

        // The head block (block4) has timestamp 1000
        // Looking backward: block3 has 1000 (count), block2 has 1000 (count), block1 has 999 (stop)
        // So number_of_blocks_with_this_timestamp = 2 (block3 and block2)
        let result = blockchain.estimate_timestamp_ms(100);
        // Should be: 1000 * 1000 + 2 * 100 = 1,000,000 + 200 = 1,000,200 ms
        assert_eq!(result, 1_000_200);
    }

    #[test]
    fn test_estimate_timestamp_ms_edge_case_single_block_zero_interval() {
        // Edge case: zero block interval
        let validators = vec![Address::from([1u8; 20])];
        let genesis = create_test_block(0, 500, validators);
        let blockchain = Blockchain::new(VecDeque::from([genesis]));

        let result = blockchain.estimate_timestamp_ms(0);
        // Should be: 500 * 1000 + 0 * 0 = 500,000 ms
        assert_eq!(result, 500_000);
    }

    #[test]
    fn test_estimate_timestamp_ms_large_numbers() {
        // Test with large timestamp and many blocks
        let validators = vec![Address::from([1u8; 20])];
        let base_timestamp = 1_640_995_200; // Jan 1, 2022 in Unix timestamp

        // Create 5 blocks with the same large timestamp
        let mut blocks = VecDeque::new();
        for i in 0..5 {
            blocks.push_back(create_test_block(i, base_timestamp, validators.clone()));
        }

        let blockchain = Blockchain::new(blocks);

        // 4 previous blocks with same timestamp as head
        // 50ms intervals
        // Should be: base_timestamp * 1000 + 4 * 50 = base_timestamp * 1000 + 200
        let result = blockchain.estimate_timestamp_ms(50);
        let expected = (base_timestamp as u128) * 1000 + 200;
        assert_eq!(result, expected);
    }
}

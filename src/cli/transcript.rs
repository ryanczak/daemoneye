//! Client-side record of everything the chat client renders.
//!
//! The inline renderer commits panels into terminal scrollback, where they are
//! frozen; this store keeps the same content in a form the alt-screen
//! transcript viewer can re-render, expand and search. See
//! `docs/design/transcript-view.md`.

/// One rendered unit of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// The user's own turn, as echoed into the transcript.
    UserTurn { label: String, text: String },
    /// Assistant prose for one turn, accumulated from `Response::Token`.
    Assistant { text: String },
    /// A tool panel header (the `▸ summary` line and its runtime label).
    ToolPanel {
        tool: String,
        summary: String,
        label: Option<String>,
    },
    /// Captured tool output, in full.
    Output {
        tool_call_id: String,
        /// The untruncated wire payload.
        full: String,
        /// How many lines the inline renderer displayed.
        shown: usize,
    },
    /// A daemon system message (`⚙ …`).
    System { text: String },
}

impl Block {
    /// Byte length of the block's own text, for the store's byte budget.
    pub fn byte_len(&self) -> usize {
        match self {
            Block::UserTurn { label, text } => label.len() + text.len(),
            Block::Assistant { text } => text.len(),
            Block::ToolPanel {
                tool,
                summary,
                label,
            } => tool.len() + summary.len() + label.as_deref().map_or(0, str::len),
            Block::Output {
                tool_call_id: id,
                full,
                ..
            } => id.len() + full.len(),
            Block::System { text } => text.len(),
        }
    }
}

/// Default cap on retained blocks.
pub const MAX_BLOCKS: usize = 500;
/// Default cap on retained block bytes.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// A bounded, ordered record of the session's rendered blocks.
#[derive(Debug, Default)]
pub struct Transcript {
    blocks: Vec<Block>,
    max_blocks: usize,
    max_bytes: usize,
    bytes: usize,
    /// Blocks evicted since construction, so the viewer can say so.
    evicted: usize,
}

impl Transcript {
    pub fn new() -> Self {
        Self::with_caps(MAX_BLOCKS, MAX_BYTES)
    }
    pub fn with_caps(max_blocks: usize, max_bytes: usize) -> Self {
        Self {
            blocks: Vec::new(),
            max_blocks,
            max_bytes,
            bytes: 0,
            evicted: 0,
        }
    }
    pub fn push(&mut self, block: Block) {
        self.bytes += block.byte_len();
        self.blocks.push(block);
        self.evict();
    }
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }
    pub fn len(&self) -> usize {
        self.blocks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
    pub fn evicted(&self) -> usize {
        self.evicted
    }
    /// Append text to the trailing `Assistant` block, or start one.
    pub fn append_assistant(&mut self, text: &str) {
        if let Some(Block::Assistant { text: existing }) = self.blocks.last_mut() {
            self.bytes += text.len();
            existing.push_str(text);
        } else {
            self.bytes += text.len();
            self.blocks.push(Block::Assistant {
                text: text.to_string(),
            });
        }
        self.evict_assistant();
    }
    fn evict(&mut self) {
        while self.blocks.len() > self.max_blocks || self.bytes > self.max_bytes {
            if self.blocks.is_empty() {
                break;
            }
            let removed = self.blocks.remove(0);
            self.bytes = self.bytes.saturating_sub(removed.byte_len());
            self.evicted += 1;
        }
    }
    /// Enforce the store budgets on the coalescing path without evicting the
    /// block being appended to when it is the only block left.
    fn evict_assistant(&mut self) {
        while self.blocks.len() > 1
            && (self.bytes > self.max_bytes || self.blocks.len() > self.max_blocks)
        {
            let removed = self.blocks.remove(0);
            self.bytes = self.bytes.saturating_sub(removed.byte_len());
            self.evicted += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Block, Transcript};

    #[test]
    fn transcript_push_evicts_oldest_over_block_cap() {
        let mut t = Transcript::with_caps(3, usize::MAX);
        for i in 0..5 {
            t.push(Block::System {
                text: format!("block {i}"),
            });
        }
        assert_eq!(t.len(), 3);
        assert_eq!(t.evicted(), 2);
        assert_eq!(
            t.blocks()[0],
            Block::System {
                text: "block 2".to_string()
            }
        );
    }

    #[test]
    fn transcript_push_evicts_over_byte_cap() {
        let mut t = Transcript::with_caps(usize::MAX, 100);
        for i in 0..3 {
            t.push(Block::System {
                text: format!("x{}:{}", "a".repeat(55), i),
            });
        }
        assert_eq!(t.len(), 1, "byte budget should force eviction");
        assert_eq!(
            t.blocks()[0],
            Block::System {
                text: format!("x{}:2", "a".repeat(55)),
            }
        );
    }

    #[test]
    fn transcript_append_assistant_coalesces() {
        let mut t = Transcript::new();
        t.append_assistant("hello ");
        t.append_assistant("world ");
        t.append_assistant("again");
        assert_eq!(t.len(), 1);
        match &t.blocks()[0] {
            Block::Assistant { text } => assert_eq!(text, "hello world again"),
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn transcript_append_assistant_breaks_on_other_block() {
        let mut t = Transcript::new();
        t.append_assistant("a");
        t.push(Block::System {
            text: "s".to_string(),
        });
        t.append_assistant("b");
        assert_eq!(t.len(), 3);
        match &t.blocks()[0] {
            Block::Assistant { text } => assert_eq!(text, "a"),
            other => panic!("expected Assistant, got {other:?}"),
        }
        match &t.blocks()[2] {
            Block::Assistant { text } => assert_eq!(text, "b"),
            other => panic!("expected Assistant, got {other:?}"),
        }
    }

    #[test]
    fn transcript_records_full_output_not_truncated() {
        let mut t = Transcript::new();
        let mut full = String::new();
        for i in 0..500 {
            full.push_str(&format!("line {i}\n"));
        }
        t.push(Block::Output {
            tool_call_id: "toolu_abc".to_string(),
            full: full.clone(),
            shown: 9,
        });
        match &t.blocks()[0] {
            Block::Output { full: stored, .. } => {
                assert_eq!(stored.lines().count(), 500);
                assert_eq!(stored, &full);
            }
            other => panic!("expected Output, got {other:?}"),
        }
    }

    #[test]
    fn append_assistant_enforces_byte_cap() {
        let mut t = Transcript::with_caps(usize::MAX, 64);
        t.push(Block::System {
            text: "x".repeat(60),
        });
        t.append_assistant(&"y".repeat(200));
        let last = t.blocks().last().expect("assistant block must survive");
        match last {
            Block::Assistant { text } => assert_eq!(text.len(), 200),
            other => panic!("expected Assistant, got {other:?}"),
        }
        assert!(
            t.bytes <= 64 || t.len() <= 1,
            "byte accounting must stay bounded"
        );
        assert_eq!(
            t.len(),
            1,
            "the oversized assistant must evict the older block"
        );
        // evicted counter advanced because the System block was evicted.
        assert_eq!(t.evicted(), 1);
    }
}

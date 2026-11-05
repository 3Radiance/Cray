use std::collections::HashMap;
use std::collections::hash_map::Entry;

use bytes::Bytes;
pub struct Reorder {
    next: u64,
    pending: HashMap<u64, Bytes>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReorderError {
    InvalidSeq,
    SeqExists,
}

impl Reorder {
    pub fn new() -> Self {
        Self {
            next: 0,
            pending: HashMap::new(),
        }
    }

    pub fn push(&mut self, seq: u64, buf: Bytes) -> Result<Vec<Bytes>, ReorderError> {
        let mut vec = Vec::new();
        if seq == self.next {
            self.next += 1;
            vec.push(buf);
            while let Some(b) = self.pending.remove(&self.next) {
                self.next += 1;
                vec.push(b);
            }
            return Ok(vec);
        } else if seq > self.next {
            match self.pending.entry(seq) {
                Entry::Occupied(_) => {
                    return Err(ReorderError::SeqExists);
                }
                Entry::Vacant(slot) => slot.insert(buf),
            };
            return Ok(vec);
        } else {
            return Err(ReorderError::InvalidSeq);
        }
    }
}

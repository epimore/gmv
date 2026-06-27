use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct BoundedQueue<T> {
    capacity: usize,
    items: VecDeque<T>,
    dropped: u64,
}

impl<T> BoundedQueue<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            items: VecDeque::new(),
            dropped: 0,
        }
    }

    pub fn push_drop_oldest(&mut self, value: T) {
        if self.items.len() == self.capacity {
            self.items.pop_front();
            self.dropped += 1;
        }
        self.items.push_back(value);
    }

    pub fn try_push(&mut self, value: T) -> Result<(), T> {
        if self.items.len() == self.capacity {
            return Err(value);
        }
        self.items.push_back(value);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

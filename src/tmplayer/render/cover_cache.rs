use std::collections::{HashMap, VecDeque, hash_map::Entry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoverKey {
    pub hash: u64,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Default)]
pub struct CoverCache {
    cap: usize,
    order: VecDeque<CoverKey>,
    map: HashMap<CoverKey, String>,
}

impl CoverCache {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            order: VecDeque::new(),
            map: HashMap::new(),
        }
    }

    pub fn get(&mut self, key: CoverKey) -> Option<String> {
        let val = self.map.get(&key)?.clone();
        self.touch(key);
        Some(val)
    }

    pub fn contains(&self, key: CoverKey) -> bool {
        self.map.contains_key(&key)
    }

    pub fn put(&mut self, key: CoverKey, val: String) {
        if let Entry::Occupied(mut e) = self.map.entry(key) {
            e.insert(val);
            self.touch(key);
            return;
        }

        self.map.insert(key, val);
        self.order.push_back(key);

        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    fn touch(&mut self, key: CoverKey) {
        if let Some(pos) = self.order.iter().position(|k| *k == key) {
            self.order.remove(pos);
            self.order.push_back(key);
        }
    }
}

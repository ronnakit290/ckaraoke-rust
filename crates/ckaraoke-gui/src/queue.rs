//! The play queue: the songs waiting to play, in order. Pure logic, so it is tested without a window.

use std::collections::VecDeque;

/// One song, as the queue and the library refer to it: the argument `load_song` takes, and what to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// A file path, or `pack.ckr#<n>` for a song in a pack (as `load_song` reads it).
    pub arg: String,
    pub title: String,
    pub artist: String,
    pub key: String,
}

#[derive(Debug, Default)]
pub struct Queue {
    items: VecDeque<Entry>,
}

impl Queue {
    /// Adds a song at the end. A song already in the queue moves to the end, as the C++ app does.
    pub fn push_back(&mut self, entry: Entry) {
        self.items.retain(|e| e.arg != entry.arg);
        self.items.push_back(entry);
    }

    /// Puts a song back at the front (used when going back to the previous song).
    pub fn push_front(&mut self, entry: Entry) {
        self.items.retain(|e| e.arg != entry.arg);
        self.items.push_front(entry);
    }

    pub fn pop_front(&mut self) -> Option<Entry> {
        self.items.pop_front()
    }

    pub fn remove(&mut self, index: usize) -> Option<Entry> {
        self.items.remove(index)
    }

    /// Drops the song with this path, if it is queued. Used when it starts playing.
    pub fn remove_song(&mut self, arg: &str) {
        self.items.retain(|e| e.arg != arg);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(arg: &str) -> Entry {
        Entry { arg: arg.into(), title: arg.into(), artist: String::new(), key: String::new() }
    }

    #[test]
    fn a_repeated_song_moves_to_the_end() {
        let mut q = Queue::default();
        q.push_back(e("a"));
        q.push_back(e("b"));
        q.push_back(e("a"));
        let order: Vec<&str> = q.iter().map(|x| x.arg.as_str()).collect();
        assert_eq!(order, vec!["b", "a"]);
    }

    #[test]
    fn a_song_that_starts_leaves_the_queue() {
        let mut q = Queue::default();
        for s in ["a", "b", "c"] {
            q.push_back(e(s));
        }
        q.remove_song("b");
        let order: Vec<&str> = q.iter().map(|x| x.arg.as_str()).collect();
        assert_eq!(order, vec!["a", "c"]);
    }

    #[test]
    fn pop_and_remove_keep_the_order() {
        let mut q = Queue::default();
        for s in ["a", "b", "c", "d"] {
            q.push_back(e(s));
        }
        assert_eq!(q.pop_front().map(|x| x.arg), Some("a".to_string()));
        assert_eq!(q.remove(1).map(|x| x.arg), Some("c".to_string()));
        let order: Vec<&str> = q.iter().map(|x| x.arg.as_str()).collect();
        assert_eq!(order, vec!["b", "d"]);
    }

    #[test]
    fn push_front_puts_a_song_first() {
        let mut q = Queue::default();
        q.push_back(e("b"));
        q.push_front(e("a"));
        assert_eq!(q.iter().next().map(|x| x.arg.as_str()), Some("a"));
        assert_eq!(q.len(), 2);
    }
}

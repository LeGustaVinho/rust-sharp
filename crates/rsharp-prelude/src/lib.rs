//! Standard prelude for R# (R Sharp) programs.
//!
//! Preserves all native Rust stdlib names while exposing C#-friendly aliases
//! and minimal zero-cost ergonomic helpers.

#![allow(dead_code, non_snake_case)]

pub use std::any::Any;
pub use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, LinkedList, VecDeque};

/// C#-friendly alias for `std::vec::Vec<T>`.
pub type List<T> = Vec<T>;

/// C#-friendly alias for `std::collections::VecDeque<T>`.
pub type Queue<T> = VecDeque<T>;

/// C#-friendly alias for `std::collections::HashMap<K, V>`.
pub type Dictionary<K, V> = HashMap<K, V>;

/// C#-friendly alias for `std::collections::BTreeMap<K, V>`.
pub type SortedDictionary<K, V> = BTreeMap<K, V>;

/// C#-friendly alias for `std::collections::BTreeSet<T>`.
pub type SortedSet<T> = BTreeSet<T>;

/// C#-friendly alias for `std::collections::BinaryHeap<T>`.
pub type PriorityQueue<T> = BinaryHeap<T>;

/// Runtime dynamic value backed by Rust's `Box<dyn Any>`.
pub type Dynamic = Box<dyn Any>;

/// Boxes any `'static` value into an R# `dynamic` value.
#[inline]
pub fn ToDynamic<T: Any + 'static>(value: T) -> Dynamic {
    Box::new(value)
}

/// Attempts to borrow a `dynamic` value as `&T`.
#[inline]
pub fn AsDynamic<T: Any + 'static>(value: &Dynamic) -> Option<&T> {
    value.downcast_ref::<T>()
}

/// Prints a formatted displayable value followed by a newline.
#[inline]
pub fn PrintLine<T: std::fmt::Display>(value: T) {
    println!("{value}");
}

/// Prints a formatted displayable value without a trailing newline.
#[inline]
pub fn Print<T: std::fmt::Display>(value: T) {
    print!("{value}");
}

/// Minimal C#-style helper methods on `List<T>` (`Vec<T>`).
pub trait RSharpListExt<T> {
    fn Add(&mut self, item: T);
    fn Count(&self) -> usize;
    fn IsEmpty(&self) -> bool;
}

impl<T> RSharpListExt<T> for Vec<T> {
    #[inline]
    fn Add(&mut self, item: T) {
        self.push(item);
    }

    #[inline]
    fn Count(&self) -> usize {
        self.len()
    }

    #[inline]
    fn IsEmpty(&self) -> bool {
        self.is_empty()
    }
}

/// Minimal C#-style helper methods on `Queue<T>` (`VecDeque<T>`).
pub trait RSharpQueueExt<T> {
    fn Enqueue(&mut self, item: T);
    fn Dequeue(&mut self) -> Option<T>;
    fn Peek(&self) -> Option<&T>;
    fn Count(&self) -> usize;
}

impl<T> RSharpQueueExt<T> for VecDeque<T> {
    #[inline]
    fn Enqueue(&mut self, item: T) {
        self.push_back(item);
    }

    #[inline]
    fn Dequeue(&mut self) -> Option<T> {
        self.pop_front()
    }

    #[inline]
    fn Peek(&self) -> Option<&T> {
        self.front()
    }

    #[inline]
    fn Count(&self) -> usize {
        self.len()
    }
}

/// Minimal C#-style helper methods on `Dictionary<K, V>` (`HashMap<K, V>`).
pub trait RSharpDictionaryExt<K, V> {
    fn Add(&mut self, key: K, value: V) -> Option<V>;
    fn ContainsKey(&self, key: &K) -> bool;
    fn Count(&self) -> usize;
}

impl<K: Eq + std::hash::Hash, V> RSharpDictionaryExt<K, V> for HashMap<K, V> {
    #[inline]
    fn Add(&mut self, key: K, value: V) -> Option<V> {
        self.insert(key, value)
    }

    #[inline]
    fn ContainsKey(&self, key: &K) -> bool {
        self.contains_key(key)
    }

    #[inline]
    fn Count(&self) -> usize {
        self.len()
    }
}

/// Minimal C#-style helper methods on strings and string slices.
pub trait RSharpStrExt {
    fn Length(&self) -> usize;
    fn ToString(&self) -> String;
}

impl RSharpStrExt for str {
    #[inline]
    fn Length(&self) -> usize {
        self.len()
    }

    #[inline]
    fn ToString(&self) -> String {
        self.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_queue_dictionary_and_dynamic_helpers() {
        let mut list: List<i32> = List::new();
        assert!(list.IsEmpty());
        list.Add(10);
        list.Add(20);
        assert_eq!(list.Count(), 2);
        assert!(!list.IsEmpty());

        let mut queue: Queue<i32> = Queue::new();
        queue.Enqueue(1);
        queue.Enqueue(2);
        assert_eq!(queue.Count(), 2);
        assert_eq!(queue.Peek(), Some(&1));
        assert_eq!(queue.Dequeue(), Some(1));
        assert_eq!(queue.Dequeue(), Some(2));
        assert_eq!(queue.Dequeue(), None);

        let mut dict: Dictionary<String, i32> = Dictionary::new();
        assert_eq!(dict.Add("a".ToString(), 42), None);
        assert!(dict.ContainsKey(&"a".ToString()));
        assert_eq!(dict.Count(), 1);
        assert_eq!("hello".Length(), 5);

        let dyn_val: Dynamic = ToDynamic(99_i32);
        assert_eq!(AsDynamic::<i32>(&dyn_val), Some(&99));
        assert_eq!(AsDynamic::<String>(&dyn_val), None);

        Print("");
        PrintLine("ok");
    }
}

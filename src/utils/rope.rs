use serde::{Deserialize, Serialize};
use std::ops::Range;

const LEAF_MIN: usize = 64;
const LEAF_MAX: usize = 512;
const MAX_DEPTH: u8 = 12;
const CONSOLIDATE_THRESHOLD: usize = 512;

#[derive(Debug, Clone)]
pub struct Rope {
    root: RopeNode,
    length: usize,
    byte_length: usize,
}

#[derive(Debug, Clone)]
enum RopeNode {
    Leaf(RopeLeaf),
    Branch(RopeBranch),
}

#[derive(Debug, Clone)]
struct RopeLeaf {
    data: String,
    char_len: usize,
}

#[derive(Debug, Clone)]
struct RopeBranch {
    left: Box<RopeNode>,
    right: Box<RopeNode>,
    weight: usize,
    depth: u8,
    char_len: usize,
    byte_len: usize,
}

#[derive(Debug, Clone, Default)]
struct LineBreakIndex {
    breaks: Vec<(usize, usize)>,
}

impl Rope {
    pub fn new() -> Self {
        Rope { root: RopeNode::Leaf(RopeLeaf { data: String::new(), char_len: 0 }), length: 0, byte_length: 0 }
    }

    pub fn from_str(s: &str) -> Self {
        if s.is_empty() { return Self::new(); }
        let char_len = s.chars().count();
        let byte_len = s.len();
        if byte_len <= LEAF_MAX * 2 {
            Rope { root: RopeNode::Leaf(RopeLeaf { data: s.to_string(), char_len }), length: char_len, byte_length: byte_len }
        } else {
            let mid = s.char_indices().nth(s.chars().count() / 2).map_or(s.len(), |(i, _)| i);
            let left = Self::from_str(&s[..mid]);
            let right = Self::from_str(&s[mid..]);
            left.concat_nodes(&right.root)
        }
    }

    fn concat_nodes(left_root: &RopeNode, right_root: &RopeNode) -> Self {
        let left_char_len = left_root.char_len();
        let right_char_len = right_root.char_len();
        let left_byte_len = left_root.byte_len();
        let right_byte_len = right_root.byte_len();
        let new_depth = 1 + left_node_depth(left_root).max(right_node_depth(right_root));
        let root = RopeNode::Branch(RopeBranch {
            left: Box::new(left_root.clone()),
            right: Box::new(right_root.clone()),
            weight: left_char_len,
            depth: new_depth,
            char_len: left_char_len + right_char_len,
            byte_len: left_byte_len + right_byte_len,
        });
        let mut rope = Rope { root, length: left_char_len + right_char_len, byte_length: left_byte_len + right_byte_len };
        if new_depth > MAX_DEPTH { rope = rope.rebalance(); }
        rope
    }

    pub fn len(&self) -> usize { self.length }

    pub fn is_empty(&self) -> bool { self.length == 0 }

    pub fn byte_len(&self) -> usize { self.byte_length }

    pub fn to_string(&self) -> String {
        let mut s = String::with_capacity(self.byte_length);
        self.root.append_to_string(&mut s);
        s
    }

    pub fn insert(&self, pos: usize, text: &str) -> Rope {
        if text.is_empty() { return self.clone(); }
        if pos >= self.length { return self.append(text); }
        let new_root = self.root.insert_at(pos, text);
        let mut rope = Rope { root: new_root, length: self.length + text.chars().count(), byte_length: self.byte_length + text.len() };
        if rope.depth() > MAX_DEPTH { rope = rope.rebalance(); }
        rope
    }

    pub fn remove(&self, range: Range<usize>) -> Rope {
        let start = range.start.min(self.length);
        let end = range.end.min(self.length);
        if start >= end || start >= self.length { return self.clone(); }
        let removed_chars = end - start;
        let removed_bytes = self.char_range_to_byte_range(start, end);
        let new_root = self.root.remove_range(start, end);
        Rope { root: new_root, length: self.length.saturating_sub(removed_chars), byte_length: self.byte_length.saturating_sub(removed_bytes) }
    }

    pub fn append(&self, text: &str) -> Rope {
        if text.is_empty() { return self.clone(); }
        if self.is_empty() { return Self::from_str(text); }
        let right = RopeNode::Leaf(RopeLeaf { data: text.to_string(), char_len: text.chars().count() });
        Self::concat_nodes(&self.root, &right)
    }

    pub fn replace(&self, range: Range<usize>, text: &str) -> Rope {
        self.remove(range.clone()).insert(range.start, text)
    }

    pub fn slice(&self, range: Range<usize>) -> Rope {
        let start = range.start.min(self.length);
        let end = range.end.min(self.length).max(start);
        if start == 0 && end == self.length { return self.clone(); }
        if start >= end { return Self::new(); }
        let new_root = self.root.slice_range(start, end);
        let sliced_chars = end - start;
        let sliced_bytes = self.char_range_to_byte_range(start, end);
        Rope { root: new_root, length: sliced_chars, byte_length: sliced_bytes }
    }

    pub fn line_count(&self) -> usize {
        self.build_line_index().line_count()
    }

    pub fn get_line(&self, line_idx: usize) -> Option<String> {
        let s = self.to_string();
        let idx = self.build_line_index();
        idx.get_line(&s, line_idx).map(|l| l.to_string())
    }

    pub fn pos_to_line(&self, char_pos: usize) -> usize {
        let idx = self.build_line_index();
        idx.pos_to_line(char_pos)
    }

    pub fn line_to_pos(&self, line_idx: usize) -> usize {
        let idx = self.build_line_index();
        idx.line_to_pos(line_idx)
    }

    pub fn lines(&self) -> Lines<'_> {
        Lines { rope: self, pos: 0 }
    }

    pub fn chars(&self) -> Chars<'_> {
        Chars { rope: self, node_stack: Vec::new(), leaf_pos: 0, initialized: false }
    }

    pub fn rebalance(&self) -> Rope {
        let leaves = self.collect_leaves();
        Self::build_balanced(&leaves)
    }

    pub fn consolidate(&self) -> Rope {
        let leaves = self.collect_leaves();
        let merged = Self::merge_small_leaves(leaves);
        Self::build_balanced(&merged)
    }

    pub fn depth(&self) -> u8 { self.node_depth(&self.root) }

    pub fn is_balanced(&self) -> bool { self.depth() <= MAX_DEPTH && self.check_balance(&self.root) }

    fn collect_leaves(&self) -> Vec<String> {
        let mut leaves = Vec::new();
        self.root.collect_leaves_into(&mut leaves);
        leaves
    }

    fn build_balanced(leaves: &[String]) -> Rope {
        if leaves.is_empty() { return Self::new(); }
        if leaves.len() == 1 {
            let s = &leaves[0];
            return Rope { root: RopeNode::Leaf(RopeLeaf { data: s.clone(), char_len: s.chars().count() }), length: s.chars().count(), byte_length: s.len() };
        }
        let mid = leaves.len() / 2;
        let left = Self::build_balanced(&leaves[..mid]);
        let right = Self::build_balanced(&leaves[mid..]);
        Self::concat_nodes(&left.root, &right.root)
    }

    fn merge_small_leaves(leaves: Vec<String>) -> Vec<String> {
        let mut result = Vec::with_capacity(leaves.len());
        let mut current = String::new();
        for leaf in leaves {
            if current.len() + leaf.len() <= CONSOLIDATE_THRESHOLD && !current.is_empty() {
                current.push_str(&leaf);
            } else {
                if !current.is_empty() { result.push(current); }
                current = leaf;
            }
        }
        if !current.is_empty() { result.push(current); }
        if result.is_empty() { result.push(String::new()); }
        result
    }

    fn build_line_index(&self) -> LineBreakIndex {
        let s = self.to_string();
        let mut breaks = Vec::new();
        let mut line_num = 0;
        breaks.push((0, line_num));
        for (byte_off, c) in s.char_indices() {
            if c == '\n' {
                line_num += 1;
                breaks.push((byte_off + 1, line_num));
            }
        }
        LineBreakIndex { breaks }
    }

    fn char_range_to_byte_range(&self, char_start: usize, char_end: usize) -> usize {
        let s = self.to_string();
        let start_byte = s.char_indices().nth(char_start).map_or(s.len(), |(i, _)| i);
        let end_byte = s.char_indices().nth(char_end).map_or(s.len(), |(i, _)| i);
        end_byte.saturating_sub(start_byte)
    }

    fn check_balance(&self, node: &RopeNode) -> bool {
        match node {
            RopeNode::Leaf(_) => true,
            RopeNode::Branch(b) => {
                let ld = left_node_depth(&b.left);
                let rd = left_node_depth(&b.right);
                (ld as i32 - rd as i32).abs() <= 1 && self.check_balance(&b.left) && self.check_balance(&b.right)
            }
        }
    }

    fn node_depth(&self, node: &RopeNode) -> u8 {
        match node {
            RopeNode::Leaf(_) => 0,
            RopeNode::Branch(b) => b.depth,
        }
    }
}


impl Default for Rope {
    fn default() -> Self { Self::new() }
}

impl PartialEq for Rope {
    fn eq(&self, other: &Self) -> bool {
        self.length == other.length && self.to_string() == other.to_string()
    }
}

impl Eq for Rope {}

impl std::fmt::Display for Rope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_string())
    }
}

impl From<&str> for Rope {
    fn from(s: &str) -> Self { Self::from_str(s) }
}

impl From<String> for Rope {
    fn from(s: String) -> Self { Self::from_str(&s) }
}

impl std::hash::Hash for Rope {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.length.hash(state);
        self.to_string().hash(state);
    }
}

unsafe impl Send for Rope {}
unsafe impl Sync for Rope {}

impl RopeNode {
    fn char_len(&self) -> usize {
        match self { RopeNode::Leaf(l) => l.char_len, RopeNode::Branch(b) => b.char_len }
    }

    fn byte_len(&self) -> usize {
        match self { RopeNode::Leaf(l) => l.data.len(), RopeNode::Branch(b) => b.byte_len }
    }

    fn insert_at(&self, pos: usize, text: &str) -> RopeNode {
        match self {
            RopeNode::Leaf(leaf) => {
                let mut data = leaf.data.clone();
                let byte_pos = data.char_indices().nth(pos).map_or(data.len(), |(i, _)| i);
                data.insert_str(byte_pos, text);
                RopeNode::Leaf(RopeLeaf { data, char_len: leaf.char_len + text.chars().count() })
            }
            RopeNode::Branch(branch) => {
                if pos <= branch.weight {
                    let new_left = branch.left.insert_at(pos, text);
                    let left_depth = left_node_depth(&new_left);
                    let right_depth = left_node_depth(&branch.right);
                    RopeNode::Branch(RopeBranch {
                        left: Box::new(new_left), right: branch.right.clone(),
                        weight: branch.weight + text.chars().count(),
                        depth: 1 + left_depth.max(right_depth),
                        char_len: branch.char_len + text.chars().count(),
                        byte_len: branch.byte_len + text.len(),
                    })
                } else {
                    let new_right = branch.right.insert_at(pos - branch.weight, text);
                    let left_depth = left_node_depth(&branch.left);
                    let right_depth = left_node_depth(&new_right);
                    RopeNode::Branch(RopeBranch {
                        left: branch.left.clone(), right: Box::new(new_right),
                        weight: branch.weight,
                        depth: 1 + left_depth.max(right_depth),
                        char_len: branch.char_len + text.chars().count(),
                        byte_len: branch.byte_len + text.len(),
                    })
                }
            }
        }
    }

    fn remove_range(&self, start: usize, end: usize) -> RopeNode {
        if start == 0 && end >= self.char_len() {
            return RopeNode::Leaf(RopeLeaf { data: String::new(), char_len: 0 });
        }
        match self {
            RopeNode::Leaf(leaf) => {
                let mut result = String::new();
                let mut chars = leaf.data.chars();
                for (_i, c) in chars.by_ref().enumerate().take(start) { result.push(c); }
                for _ in start..end { chars.next(); }
                for c in chars { result.push(c); }
                let char_len = result.chars().count();
                RopeNode::Leaf(RopeLeaf { data: result, char_len })
            }
            RopeNode::Branch(branch) => {
                if end <= branch.weight {
                    let new_left = branch.left.remove_range(start, end);
                    Self::make_branch(new_left, (*branch.right).clone())
                } else if start >= branch.weight {
                    let new_right = branch.right.remove_range(start - branch.weight, end - branch.weight);
                    Self::make_branch((*branch.left).clone(), new_right)
                } else {
                    let left_part = branch.left.slice_range(start, branch.weight);
                    let right_part = branch.right.slice_range(0, end - branch.weight);
                    Self::concat_two(left_part, right_part)
                }
            }
        }
    }

    fn slice_range(&self, start: usize, end: usize) -> RopeNode {
        if start == 0 && end >= self.char_len() { return self.clone(); }
        match self {
            RopeNode::Leaf(leaf) => {
                let mut result = String::new();
                let mut chars = leaf.data.chars();
                for (i, c) in chars.by_ref().enumerate().take(end) {
                    if i >= start { result.push(c); } else { continue; }
                }
                let char_len = result.chars().count();
                RopeNode::Leaf(RopeLeaf { data: result, char_len })
            }
            RopeNode::Branch(branch) => {
                if end <= branch.weight {
                    branch.left.slice_range(start, end)
                } else if start >= branch.weight {
                    branch.right.slice_range(start - branch.weight, end - branch.weight)
                } else {
                    let left_slice = branch.left.slice_range(start, branch.weight);
                    let right_slice = branch.right.slice_range(0, end - branch.weight);
                    Self::concat_two(left_slice, right_slice)
                }
            }
        }
    }

    fn make_branch(left: RopeNode, right: RopeNode) -> RopeNode {
        let lc = left.char_len();
        let rc = right.char_len();
        let lb = left.byte_len();
        let rb = right.byte_len();
        RopeNode::Branch(RopeBranch {
            left: Box::new(left.clone()), right: Box::new(right.clone()),
            weight: lc,
            depth: 1 + left_node_depth(&left).max(left_node_depth(&right)),
            char_len: lc + rc, byte_len: lb + rb,
        })
    }

    fn concat_two(left: RopeNode, right: RopeNode) -> RopeNode {
        if left.char_len() == 0 { return right; }
        if right.char_len() == 0 { return left; }
        Self::make_branch(left, right)
    }

    fn append_to_string(&self, s: &mut String) {
        match self {
            RopeNode::Leaf(leaf) => s.push_str(&leaf.data),
            RopeNode::Branch(branch) => { branch.left.append_to_string(s); branch.right.append_to_string(s); }
        }
    }

    fn collect_leaves_into(&self, out: &mut Vec<String>) {
        match self {
            RopeNode::Leaf(leaf) => out.push(leaf.data.clone()),
            RopeNode::Branch(branch) => { branch.left.collect_leaves_into(out); branch.right.collect_leaves_into(out); }
        }
    }
}

fn left_node_depth(node: &RopeNode) -> u8 {
    match node { RopeNode::Leaf(_) => 0, RopeNode::Branch(b) => b.depth }
}

fn right_node_depth(node: &RopeNode) -> u8 {
    match node { RopeNode::Leaf(_) => 0, RopeNode::Branch(b) => b.depth }
}

impl LineBreakIndex {
    fn line_count(&self) -> usize {
        if self.breaks.is_empty() { 1 } else { self.breaks.last().unwrap().1 + 1 }
    }

    fn get_line<'a>(&self, full_text: &'a str, line_idx: usize) -> Option<&'a str> {
        if line_idx >= self.line_count() { return None; }
        let start_byte = if line_idx == 0 { 0 } else {
            self.breaks.iter().find(|&&(_, ln)| ln == line_idx).map(|&(b, _)| b).unwrap_or(0)
        };
        let end_byte = self.breaks.iter()
            .find(|&&(_, ln)| ln == line_idx + 1)
            .map(|&(b, _)| b)
            .unwrap_or(full_text.len());
        let line = &full_text[start_byte..end_byte];
        Some(line.trim_end_matches('\n'))
    }

    fn pos_to_line(&self, char_pos: usize) -> usize {
        let mut last_line = 0;
        for &(byte_off, line_num) in &self.breaks {
            if byte_off > char_pos { break; }
            last_line = line_num;
        }
        last_line
    }

    fn line_to_pos(&self, line_idx: usize) -> usize {
        self.breaks.iter().find(|&&(_, ln)| ln == line_idx).map(|&(b, _)| b).unwrap_or(0)
    }
}

pub struct Chars<'a> {
    rope: &'a Rope,
    node_stack: Vec<(&'a RopeNode, usize)>,
    leaf_pos: usize,
    leaf_data: Option<String>,
    initialized: bool,
}

impl<'a> Iterator for Chars<'a> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        if !self.initialized {
            self.initialized = true;
            if self.rope.length > 0 { self.node_stack.push((&self.rope.root, 0)); }
        }
        loop {
            if let Some(ref data) = self.leaf_data {
                if let Some((byte_idx, _)) = data.char_indices().nth(self.leaf_pos) {
                    let c = data[byte_idx..].chars().next()?;
                    self.leaf_pos += 1;
                    return Some(c);
                }
                self.leaf_data = None;
                self.leaf_pos = 0;
            }
            match self.node_stack.pop() {
                None => return None,
                Some((RopeNode::Leaf(leaf), _)) => {
                    if !leaf.data.is_empty() {
                        self.leaf_data = Some(leaf.data.clone());
                        self.leaf_pos = 0;
                    }
                }
                Some((RopeNode::Branch(branch), _)) => {
                    self.node_stack.push((&*branch.right, 0));
                    self.node_stack.push((&*branch.left, 0));
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = if let Some(ref d) = self.leaf_data { d[self.leaf_pos..].chars().count() } else { 0 };
        (remaining, Some(self.rope.length))
    }
}

impl<'a> IntoIterator for &'a Rope {
    type Item = char;
    type IntoIter = Chars<'a>;
    fn into_iter(self) -> Chars<'a> { self.chars() }
}

pub struct Lines<'a> {
    rope: &'a Rope,
    pos: usize,
}

impl<'a> Iterator for Lines<'a> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        self.rope.get_line(self.pos).map(|line| {
            self.pos += 1;
            line
        })
    }
}

impl Serialize for Rope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Rope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: serde::Deserializer<'de> {
        let s = String::deserialize(deserializer)?;
        Ok(Rope::from_str(&s))
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_new_is_empty() {
        let r = Rope::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert_eq!(r.to_string(), "");
    }

    #[test]
    fn test_from_str_basic() {
        let r = Rope::from_str("hello");
        assert_eq!(r.len(), 5);
        assert!(!r.is_empty());
        assert_eq!(r.to_string(), "hello");
    }

    #[test]
    fn test_from_str_empty() {
        let r = Rope::from_str("");
        assert!(r.is_empty());
    }

    #[test]
    fn test_from_str_unicode() {
        let r = Rope::from_str("你好世界🌍");
        assert_eq!(r.len(), 5);
        assert_eq!(r.to_string(), "你好世界🌍");
    }

    #[test]
    fn test_insert_beginning() {
        let r = Rope::from_str("world");
        let r2 = r.insert(0, "hello ");
        assert_eq!(r2.to_string(), "hello world");
        assert_eq!(r.to_string(), "world");
    }

    #[test]
    fn test_insert_middle() {
        let r = Rope::from_str("helloworld");
        let r2 = r.insert(5, " ");
        assert_eq!(r2.to_string(), "hello world");
    }

    #[test]
    fn test_insert_end() {
        let r = Rope::from_str("hello");
        let r2 = r.insert(5, " world");
        assert_eq!(r2.to_string(), "hello world");
    }

    #[test]
    fn test_insert_empty_text() {
        let r = Rope::from_str("hello");
        let r2 = r.insert(3, "");
        assert_eq!(r2.to_string(), "hello");
    }

    #[test]
    fn test_insert_beyond_end_appends() {
        let r = Rope::from_str("hi");
        let r2 = r.insert(100, " there");
        assert_eq!(r2.to_string(), "hi there");
    }

    #[test]
    fn test_remove_middle() {
        let r = Rope::from_str("hello world");
        let r2 = r.remove(5..11);
        assert_eq!(r2.to_string(), "hello");
    }

    #[test]
    fn test_remove_beginning() {
        let r = Rope::from_str("hello world");
        let r2 = r.remove(0..6);
        assert_eq!(r2.to_string(), "world");
    }

    #[test]
    fn test_remove_end() {
        let r = Rope::from_str("hello world");
        let r2 = r.remove(6..11);
        assert_eq!(r2.to_string(), "hello ");
    }

    #[test]
    fn test_remove_all() {
        let r = Rope::from_str("hello");
        let r2 = r.remove(0..5);
        assert!(r2.is_empty());
    }

    #[test]
    fn test_remove_empty_range() {
        let r = Rope::from_str("hello");
        let r2 = r.remove(3..3);
        assert_eq!(r2.to_string(), "hello");
    }

    #[test]
    fn test_remove_out_of_bounds() {
        let r = Rope::from_str("hi");
        let r2 = r.remove(10..20);
        assert_eq!(r2.to_string(), "hi");
    }

    #[test]
    fn test_append_basic() {
        let r = Rope::from_str("hello");
        let r2 = r.append(" world");
        assert_eq!(r2.to_string(), "hello world");
    }

    #[test]
    fn test_append_empty_rope() {
        let r = Rope::new();
        let r2 = r.append("hello");
        assert_eq!(r2.to_string(), "hello");
    }

    #[test]
    fn test_append_empty_text() {
        let r = Rope::from_str("hello");
        let r2 = r.append("");
        assert_eq!(r2.to_string(), "hello");
    }

    #[test]
    fn test_replace_basic() {
        let r = Rope::from_str("hello world");
        let r2 = r.replace(6..11, "rust");
        assert_eq!(r2.to_string(), "hello rust");
    }

    #[test]
    fn test_replace_with_longer() {
        let r = Rope::from_str("hi");
        let r2 = r.replace(0..2, "hello");
        assert_eq!(r2.to_string(), "hello");
    }

    #[test]
    fn test_replace_with_shorter() {
        let r = Rope::from_str("hello");
        let r2 = r.replace(0..5, "hi");
        assert_eq!(r2.to_string(), "hi");
    }

    #[test]
    fn test_slice_full() {
        let r = Rope::from_str("hello world");
        let s = r.slice(0..11);
        assert_eq!(s.to_string(), "hello world");
    }

    #[test]
    fn test_slice_partial() {
        let r = Rope::from_str("hello world");
        let s = r.slice(0..5);
        assert_eq!(s.to_string(), "hello");
    }

    #[test]
    fn test_slice_middle() {
        let r = Rope::from_str("hello world");
        let s = r.slice(6..11);
        assert_eq!(s.to_string(), "world");
    }

    #[test]
    fn test_slice_empty_range() {
        let r = Rope::from_str("hello");
        let s = r.slice(3..3);
        assert!(s.is_empty());
    }

    #[test]
    fn test_slice_out_of_bounds() {
        let r = Rope::from_str("hi");
        let s = r.slice(0..100);
        assert_eq!(s.to_string(), "hi");
    }

    #[test]
    fn test_line_count_single() {
        let r = Rope::from_str("hello");
        assert_eq!(r.line_count(), 1);
    }

    #[test]
    fn test_line_count_multi() {
        let r = Rope::from_str("line1\nline2\nline3");
        assert_eq!(r.line_count(), 3);
    }

    #[test]
    fn test_get_line() {
        let r = Rope::from_str("first\nsecond\nthird");
        assert_eq!(r.get_line(0).as_deref(), Some("first"));
        assert_eq!(r.get_line(1).as_deref(), Some("second"));
        assert_eq!(r.get_line(2).as_deref(), Some("third"));
        assert_eq!(r.get_line(3), None);
    }

    #[test]
    fn test_pos_to_line() {
        let r = Rope::from_str("aaa\nbbb\nccc");
        assert_eq!(r.pos_to_line(0), 0);
        assert_eq!(r.pos_to_line(3), 0);
        assert_eq!(r.pos_to_line(4), 1);
        assert_eq!(r.pos_to_line(7), 1);
        assert_eq!(r.pos_to_line(8), 2);
    }

    #[test]
    fn test_line_to_pos() {
        let r = Rope::from_str("aaa\nbbb\nccc");
        assert_eq!(r.line_to_pos(0), 0);
        assert_eq!(r.line_to_pos(1), 4);
        assert_eq!(r.line_to_pos(2), 8);
    }

    #[test]
    fn test_lines_iterator() {
        let r = Rope::from_str("a\nb\nc");
        let lines: Vec<String> = r.lines().collect();
        assert_eq!(lines, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn test_chars_iterator() {
        let r = Rope::from_str("abc");
        let chars: Vec<char> = r.into_iter().collect();
        assert_eq!(chars, vec!['a', 'b', 'c']);
    }

    #[test]
    fn test_chars_iterator_empty() {
        let r = Rope::new();
        let chars: Vec<char> = r.into_iter().collect();
        assert!(chars.is_empty());
    }

    #[test]
    fn test_immutability_on_edit() {
        let r = Rope::from_str("original");
        let r2 = r.insert(0, "prefix ");
        assert_eq!(r.to_string(), "original");
        assert_eq!(r2.to_string(), "prefix original");
    }

    #[test]
    fn test_rebalance_reduces_depth() {
        let mut r = Rope::new();
        for i in 0..200u32 {
            r = r.append(&format!("chunk{}", i));
        }
        assert!(r.depth() > MAX_DEPTH || r.is_balanced());
        let balanced = r.rebalance();
        assert!(balanced.is_balanced());
        assert_eq!(balanced.to_string(), r.to_string());
    }

    #[test]
    fn test_consolidate_merges_fragments() {
        let mut r = Rope::new();
        for _ in 0..50 {
            r = r.append("small");
        }
        let consolidated = r.consolidate();
        assert_eq!(consolidated.to_string(), r.to_string());
        let leaves = consolidated.collect_leaves();
        assert!(leaves.len() < 50, "consolidation should reduce leaf count: got {}", leaves.len());
    }

    #[test]
    fn test_large_scale_inserts() {
        let mut r = Rope::new();
        for i in 0..1000 {
            r = r.insert(i, &format!("{}", i % 10));
        }
        assert_eq!(r.len(), 1000);
    }

    #[test]
    fn test_large_scale_from_str() {
        let big: String = (0..20000).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let r = Rope::from_str(&big);
        assert_eq!(r.len(), 20000);
        assert_eq!(r.to_string(), big);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let original = Rope::from_str("hello\nworld\nrust");
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: Rope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.to_string(), original.to_string());
        assert_eq!(restored.len(), original.len());
    }

    #[test]
    fn test_serialization_empty() {
        let r = Rope::new();
        let json = serde_json::to_string(&r).expect("serialize");
        let restored: Rope = serde_json::from_str(&json).expect("deserialize");
        assert!(restored.is_empty());
    }

    #[test]
    fn test_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<Rope>();
        assert_sync::<Rope>();
    }

    #[test]
    fn test_display_trait() {
        let r = Rope::from_str("hello");
        assert_eq!(format!("{}", r), "hello");
    }

    #[test]
    fn test_from_string() {
        let r = Rope::from(String::from("test"));
        assert_eq!(r.to_string(), "test");
    }

    #[test]
    fn test_equality() {
        let a = Rope::from_str("same");
        let b = Rope::from_str("same");
        let c = Rope::from_str("different");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_hash_consistent() {
        use std::collections::HashSet;
        let a = Rope::from_str("same");
        let b = Rope::from_str("same");
        let mut set = HashSet::new();
        set.insert(a);
        assert!(set.contains(&b));
    }

    #[test]
    fn test_default() {
        let r = Rope::default();
        assert!(r.is_empty());
    }

    #[test]
    fn test_multiple_edits_chain() {
        let r = Rope::from_str("abcdef");
        let r2 = r.remove(2..4);
        let r3 = r2.insert(2, "XY");
        let r4 = r3.replace(0..2, "AB");
        assert_eq!(r4.to_string(), "ABXYef");
    }

    #[test]
    fn test_newlines_in_get_line() {
        let r = Rope::from_str("line1\r\nline2\nline3\rline4");
        assert_eq!(r.line_count(), 4);
    }

    #[test]
    fn test_deeply_nested_structure_sharing() {
        let base = Rope::from_str("base text that is reasonably long for testing structure sharing behavior");
        let v1 = base.insert(0, "version1 prefix: ");
        let v2 = base.insert(0, "version2 prefix: different content here ");
        assert_ne!(v1.to_string(), v2.to_string());
        assert_eq!(base.to_string(), "base text that is reasonably long for testing structure sharing behavior");
    }

    #[test]
    fn test_stress_many_random_edits() {
        let mut r = Rope::from_str("initial string value for stress testing purposes");
        for i in 0..500 {
            let pos = (i as usize) % (r.len().max(1));
            r = r.insert(pos, &format!("@{}", i % 10));
        }
        assert!(!r.is_empty());
        let _s = r.to_string();
    }

    #[test]
    fn test_byte_len_accuracy() {
        let r = Rope::from_str("你好");
        assert_eq!(r.byte_len(), 6);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn test_append_then_slice() {
        let r = Rope::from_str("abc").append("def").append("ghi");
        let s = r.slice(3..9);
        assert_eq!(s.to_string(), "defghi");
    }
}

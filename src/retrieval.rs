//! # BM25 + 向量混合检索 — 借鉴 TencentDB-Agent-Memory 的检索策略
//!
//! 支持 BM25(关键词) + 向量嵌入 + RRF(递归排序融合) 混合检索
//! 当语义检索失败时，关键词匹配作为强有力的后备方案

use std::collections::HashMap;

/// BM25 检索器
pub struct BM25Retriever {
    /// 文档集合
    docs: Vec<String>,
    /// 文档 ID
    doc_ids: Vec<String>,
    /// 文档 -> 词频
    doc_term_freqs: Vec<HashMap<String, usize>>,
    /// 文档长度 (词数)
    doc_lengths: Vec<usize>,
    /// 包含每个 term 的文档数
    term_doc_count: HashMap<String, usize>,
    /// 平均文档长度
    avg_doc_len: f64,
    /// BM25 参数
    k1: f64,
    b: f64,
    /// 总文档数
    total_docs: usize,
}

impl BM25Retriever {
    /// 创建 BM25 检索器
    pub fn new(k1: f64, b: f64) -> Self {
        Self {
            docs: Vec::new(),
            doc_ids: Vec::new(),
            doc_term_freqs: Vec::new(),
            doc_lengths: Vec::new(),
            term_doc_count: HashMap::new(),
            avg_doc_len: 0.0,
            k1,
            b,
            total_docs: 0,
        }
    }

    /// 添加文档到索引
    pub fn add_document(&mut self, id: &str, content: &str) {
        let terms = tokenize(content);
        let mut term_freq: HashMap<String, usize> = HashMap::new();
        for term in &terms {
            *term_freq.entry(term.clone()).or_insert(0) += 1;
        }
        // 更新 term 的文档频率
        for term in term_freq.keys() {
            *self.term_doc_count.entry(term.clone()).or_insert(0) += 1;
        }
        self.doc_ids.push(id.to_string());
        self.docs.push(content.to_string());
        self.doc_term_freqs.push(term_freq);
        self.doc_lengths.push(terms.len());
        self.total_docs += 1;
        self.avg_doc_len = self.doc_lengths.iter().sum::<usize>() as f64 / self.total_docs as f64;
    }

    /// 批量添加文档
    pub fn add_documents(&mut self, docs: Vec<(String, String)>) {
        for (id, content) in docs {
            self.add_document(&id, &content);
        }
    }

    /// 搜索 — 返回 (doc_id, score) 列表
    pub fn search(&self, query: &str, top_k: usize) -> Vec<(String, f64)> {
        let query_terms = tokenize(query);
        if query_terms.is_empty() || self.total_docs == 0 {
            return Vec::new();
        }

        let mut scores: Vec<(usize, f64)> = (0..self.total_docs)
            .map(|i| (i, self.score_doc(i, &query_terms)))
            .collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(top_k);
        scores.into_iter().map(|(i, s)| (self.doc_ids[i].clone(), s)).collect()
    }

    /// 计算单个文档的 BM25 分数
    fn score_doc(&self, doc_idx: usize, query_terms: &[String]) -> f64 {
        let doc_len = self.doc_lengths[doc_idx];
        let term_freqs = &self.doc_term_freqs[doc_idx];
        let mut score = 0.0;

        for term in query_terms {
            let tf = *term_freqs.get(term).unwrap_or(&0) as f64;
            if tf == 0.0 {
                continue;
            }
            let df = *self.term_doc_count.get(term).unwrap_or(&1) as f64;
            let idf = ((self.total_docs as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();
            let numerator = tf * (self.k1 + 1.0);
            let denominator = tf + self.k1 * (1.0 - self.b + self.b * doc_len as f64 / self.avg_doc_len);
            score += idf * numerator / denominator;
        }
        score
    }
}

/// 简单分词
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| !s.is_empty() && s.len() >= 2)
        .map(String::from)
        .collect()
}

/// RRF (Reciprocal Rank Fusion) — 混合 BM25 和向量检索结果
pub fn reciprocal_rank_fusion(
    bm25_results: &[(String, f64)],
    vector_results: &[(String, f32)],
    k: f64,
    top_k: usize,
) -> Vec<(String, f64)> {
    let mut scores: HashMap<String, f64> = HashMap::new();

    for (i, (doc_id, _)) in bm25_results.iter().enumerate() {
        let rank = (i + 1) as f64;
        *scores.entry(doc_id.clone()).or_insert(0.0) += 1.0 / (k + rank);
    }

    for (i, (doc_id, _)) in vector_results.iter().enumerate() {
        let rank = (i + 1) as f64;
        *scores.entry(doc_id.clone()).or_insert(0.0) += 1.0 / (k + rank);
    }

    let mut result: Vec<(String, f64)> = scores.into_iter().collect();
    result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    result.truncate(top_k);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bm25_basic() {
        let mut retriever = BM25Retriever::new(1.5, 0.75);
        retriever.add_document("doc1", "The quick brown fox jumps over the lazy dog");
        retriever.add_document("doc2", "A fast brown fox leaps over a sleepy dog");
        retriever.add_document("doc3", "The cat sat on the mat and looked at the dog");

        let results = retriever.search("brown fox", 2);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "doc1");
        assert_eq!(results[1].0, "doc2");
    }

    #[test]
    fn test_rrf_merge() {
        let bm25 = vec![("doc1".into(), 2.5), ("doc2".into(), 1.5)];
        let vec = vec![("doc2".into(), 0.9), ("doc3".into(), 0.8)];
        let merged = reciprocal_rank_fusion(&bm25, &vec, 60.0, 3);
        assert_eq!(merged.len(), 3);
        // doc2 在两个列表中都有，应该排第一
        assert_eq!(merged[0].0, "doc2");
    }
}

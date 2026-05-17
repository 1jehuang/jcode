//! 基于 tantivy 的语义索引引擎

use anyhow::Result;
use tantivy::{
    collector::TopDocs,
    query::QueryParser,
    schema::{Schema, TEXT, STORED, INDEXED},
    Index, IndexWriter, ReloadPolicy,
};
use std::path::PathBuf;
use tokio::sync::Mutex;

use crate::parser::Symbol;

/// 搜索结果
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file_path: String,
    pub symbol_name: String,
    pub content: String,
    pub score: f32,
}

/// 语义索引器
pub struct SemanticIndexer {
    index: Index,
    schema: Schema,
    writer: Mutex<IndexWriter>,
}

impl SemanticIndexer {
    pub fn new(index_path: PathBuf) -> Result<Self> {
        // 定义索引结构 (Schema)
        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field("file_path", STORED);
        schema_builder.add_text_field("symbol_name", TEXT | STORED);
        schema_builder.add_text_field("content", TEXT);
        schema_builder.add_u64_field("start_line", INDEXED);
        
        let schema = schema_builder.build();

        // 创建或打开索引
        let index = Index::open_in_dir(&index_path).unwrap_or_else(|_| {
            std::fs::create_dir_all(&index_path).ok();
            Index::create_in_dir(&index_path, schema.clone()).unwrap()
        });

        let writer = index.writer(50_000_000)?; // 50MB buffer

        Ok(Self {
            index,
            schema,
            writer: Mutex::new(writer),
        })
    }

    /// 添加文档到索引
    pub async fn add_document(&self, file_path: &str, symbols: &[Symbol], full_content: &str) -> Result<()> {
        let mut writer = self.writer.lock().await;
        
        for symbol in symbols {
            let doc = tantivy::schema::Document::default();
            let mut doc = doc;
            doc.add_text(self.schema.get_field("file_path").unwrap(), file_path);
            doc.add_text(self.schema.get_field("symbol_name").unwrap(), symbol.name.as_str());
            doc.add_text(self.schema.get_field("content").unwrap(), symbol.content.as_str());
            doc.add_u64(self.schema.get_field("start_line").unwrap(), symbol.start_line as u64);
            writer.add_document(doc)?;
        }

        writer.commit()?;
        Ok(())
    }

    /// 搜索代码
    pub async fn search(&self, query_str: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let reader = self.index.reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        let searcher = reader.searcher();
        let schema = self.schema.clone();
        
        let query_parser = QueryParser::for_index(
            &self.index,
            vec![
                schema.get_field("symbol_name").unwrap(),
                schema.get_field("content").unwrap(),
            ],
        );

        let query = query_parser.parse_query(query_str)?;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(limit))?;

        let mut results = Vec::new();
        for (score, doc_address) in top_docs {
            let retrieved_doc: tantivy::schema::Document = searcher.doc(doc_address)?;
            let file_path = retrieved_doc.get_first(schema.get_field("file_path").unwrap())
                .and_then(|v| v.as_text())
                .unwrap_or("").to_string();
            
            let symbol_name = retrieved_doc.get_first(schema.get_field("symbol_name").unwrap())
                .and_then(|v: &OwnedValue| v.as_text())
                .unwrap_or("").to_string();

            let content = retrieved_doc.get_first(schema.get_field("content").unwrap())
                .and_then(|v: &OwnedValue| v.as_text())
                .unwrap_or("").to_string();

            results.push(SearchResult {
                file_path,
                symbol_name,
                content,
                score,
            });
        }

        Ok(results)
    }
}

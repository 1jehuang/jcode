// ════════════════════════════════════════════════════════════════
// Jupyter Notebook 编辑器 — .ipynb 文件的读写/编辑/执行
//
// 支持:
//   - 读取/写入 .ipynb (JSON 格式)
//   - Cell 管理 (添加/删除/重排序/合并)
//   - Code / Markdown / Raw cell 类型
//   - 输出管理 (text/image/error)
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellType {
    Code,
    Markdown,
    Raw,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookCell {
    pub id: String,
    pub cell_type: CellType,
    pub source: Vec<String>, // 每行一个字符串
    pub outputs: Vec<CellOutput>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub execution_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellOutput {
    pub output_type: OutputType,
    pub text: Option<String>,
    pub data: Option<HashMap<String, String>>, // MIME type -> base64 content
    pub execution_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputType {
    Stream,      // stdout 流
    DisplayData, // 富文本显示 (image/html)
    ExecuteResult, // 执行结果
    Error,       // 错误输出
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookMetadata {
    pub kernelspec: Option<KernelSpec>,
    pub language_info: Option<LanguageInfo>,
    pub author: Option<String>,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSpec { pub name: String, pub display_name: String, pub language: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageInfo { pub name: String, pub version: Option<String> }

/// Jupyter Notebook 完整结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notebook {
    pub nbformat: u32,
    pub nbformat_minor: u32,
    pub metadata: NotebookMetadata,
    pub cells: Vec<NotebookCell>,
}

impl Default for Notebook {
    fn default() -> Self {
        Self::new("python")
    }
}

impl Notebook {
    pub fn new(language: &str) -> Self {
        Self {
            nbformat: 4,
            nbformat_minor: 5,
            metadata: NotebookMetadata {
                kernelspec: Some(KernelSpec {
                    name: format!("{}-jcode", language),
                    display_name: language.to_string(),
                    language: language.to_string(),
                }),
                language_info: Some(LanguageInfo {
                    name: language.to_string(),
                    version: Some("3.12.0".into()),
                }),
                author: Some("JCode".into()),
                created_at: Some(chrono::Utc::now().to_rfc3339()),
            },
            cells: vec![],
        }
    }

    /// 从文件读取
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self, std::io::Error> {
        let content = std::fs::read_to_string(path)?;
        let nb: Notebook = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(nb)
    }

    /// 写入文件
    pub fn to_file(&self, path: impl AsRef<std::path::Path>) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)
    }

    /// 添加代码 cell
    pub fn add_code_cell(&mut self, code: &str) -> usize {
        let index = self.cells.len();
        self.cells.push(NotebookCell {
            id: uuid::Uuid::new_v4().to_string(),
            cell_type: CellType::Code,
            source: code.lines().map(String::from).collect(),
            outputs: vec![],
            metadata: HashMap::new(),
            execution_count: None,
        });
        index
    }

    /// 添加 markdown cell
    pub fn add_markdown_cell(&mut self, text: &str) -> usize {
        let index = self.cells.len();
        self.cells.push(NotebookCell {
            id: uuid::Uuid::new_v4().to_string(),
            cell_type: CellType::Markdown,
            source: text.lines().map(String::from).collect(),
            outputs: vec![],
            metadata: HashMap::new(),
            execution_count: None,
        });
        index
    }

    /// 删除 cell
    pub fn remove_cell(&mut self, index: usize) -> bool {
        if index < self.cells.len() {
            self.cells.remove(index);
            true
        } else { false }
    }

    /// 获取所有 code cells 的源码
    pub fn code_cells(&self) -> Vec<&NotebookCell> {
        self.cells.iter()
            .filter(|c| c.cell_type == CellType::Code)
            .collect()
    }
}

/// Notebook 编辑器 (高级操作)
pub struct NotebookEditor {
    notebook: Notebook,
}

impl NotebookEditor {
    pub fn new(notebook: Notebook) -> Self {
        Self { notebook }
    }

    pub fn into_inner(self) -> Notebook {
        self.notebook
    }

    /// 合并相邻的同类 cell
    pub fn merge_adjacent_cells(&mut self) -> usize {
        let mut merged = 0;
        let mut i = 0;
        
        while i < self.notebook.cells.len().saturating_sub(1) {
            if self.notebook.cells[i].cell_type == self.notebook.cells[i + 1].cell_type {
                let next_source = self.notebook.cells.remove(i + 1);
                if let Some(current) = self.notebook.cells.get_mut(i) {
                    current.source.extend(next_source.source);
                    merged += 1;
                }
            } else {
                i += 1;
            }
        }

        merged
    }

    /// 清空所有 cell 输出
    pub fn clear_outputs(&mut self) {
        for cell in &mut self.notebook.cells {
            cell.outputs.clear();
            cell.execution_count = None;
        }
    }

    /// 统计信息
    pub fn stats(&self) -> NotebookStats {
        let total = self.notebook.cells.len();
        let code = self.notebook.code_cells().len();
        let md = total - code;
        
        NotebookStats { total_cells: total, code_cells: code, markdown_cells: md }
    }
}

#[derive(Debug, Clone)]
pub struct NotebookStats {
    pub total_cells: usize,
    pub code_cells: usize,
    pub markdown_cells: usize,
}

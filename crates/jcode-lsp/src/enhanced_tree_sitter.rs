//! Enhanced Tree-sitter with Control Flow Graph Support
//!
//! 在原有 tree_sitter.rs 基础上增加：
//! - 控制流图 (CFG) 构建
//! - 数据流分析基础
//! - 循环检测
//! - 复杂度度量

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

// 重新导出原有类型
pub use crate::tree_sitter::{
    LanguageId, NodeType, SourceLocation, AstNode, TypeInfo,
    SymbolEntry, SymbolKind, ParseResult, ParserConfig,
    ParseError, TreeSitterParserManager,
};

/// 基本块 ID
pub type BlockId = usize;

/// 边 ID
pub type EdgeId = usize;

/// 控制流图节点 (基本块)
#[derive(Debug, Clone)]
pub struct BasicBlock {
    /// 块 ID
    pub id: BlockId,
    
    /// 包含的 AST 节点 ID 列表
    pub node_ids: Vec<u64>,
    
    /// 入边 (哪些块可以跳转到这个块)
    pub predecessors: Vec<BlockId>,
    
    /// 出边 (这个块可以跳转到哪些块)
    pub successors: Vec<BlockId>,
    
    /// 是否是入口块
    pub is_entry: bool,
    
    /// 是否是出口块
    pub is_exit: bool,
}

impl BasicBlock {
    pub fn new(id: BlockId) -> Self {
        Self {
            id,
            node_ids: Vec::new(),
            predecessors: Vec::new(),
            successors: Vec::new(),
            is_entry: false,
            is_exit: false,
        }
    }

    /// 添加后继块
    pub fn add_successor(&mut self, block_id: BlockId) {
        if !self.successors.contains(&block_id) {
            self.successors.push(block_id);
        }
    }

    /// 添加前驱块
    pub fn add_predecessor(&mut self, block_id: BlockId) {
        if !self.predecessors.contains(&block_id) {
            self.predecessors.push(block_id);
        }
    }
}

/// CFG 边
#[derive(Debug, Clone)]
pub struct CFGEdge {
    /// 边 ID
    pub id: EdgeId,
    
    /// 源块
    pub from: BlockId,
    
    /// 目标块
    pub to: BlockId,
    
    /// 边类型
    pub edge_type: EdgeType,
    
    /// 条件 (对于条件分支)
    pub condition: Option<String>,
}

/// 边类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeType {
    /// 无条件跳转 (fall-through, goto)
    Unconditional,
    /// 真分支 (if condition is true)
    TrueBranch,
    /// 假分支 (if condition is false)
    FalseBranch,
    /// case 分支
    CaseBranch(String),
    /// 循环回边 (back edge)
    LoopBack,
    /// 异常/错误处理
    Exception,
    /// 函数返回
    Return,
}

/// 控制流图
#[derive(Debug, Clone)]
pub struct ControlFlowGraph {
    /// 所有基本块
    pub blocks: HashMap<BlockId, BasicBlock>,
    
    /// 所有边
    pub edges: Vec<CFGEdge>,
    
    /// 入口块 ID
    pub entry_block: Option<BlockId>,
    
    /// 出口块 IDs
    pub exit_blocks: Vec<BlockId>,
    
    /// 支配树 (用于循环检测)
    pub dominator_tree: Option<DominatorTree>,
    
    /// 检测到的循环
    pub loops: Vec<LoopInfo>,
}

impl Default for ControlFlowGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlFlowGraph {
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            edges: Vec::new(),
            entry_block: None,
            exit_blocks: Vec::new(),
            dominator_tree: None,
            loops: Vec::new(),
        }
    }

    /// 创建新基本块
    pub fn create_block(&mut self) -> BlockId {
        let id = self.blocks.len();
        let block = BasicBlock::new(id);
        self.blocks.insert(id, block);
        id
    }

    /// 添加边
    pub fn add_edge(&mut self, from: BlockId, to: BlockId, edge_type: EdgeType) -> EdgeId {
        let edge_id = self.edges.len();
        
        // 更新块的邻接关系
        if let Some(from_block) = self.blocks.get_mut(&from) {
            from_block.add_successor(to);
        }
        if let Some(to_block) = self.blocks.get_mut(&to) {
            to_block.add_predecessor(from);
        }
        
        let edge = CFGEdge {
            id: edge_id,
            from,
            to,
            edge_type,
            condition: None,
        };
        
        self.edges.push(edge);
        edge_id
    }

    /// 设置入口块
    pub fn set_entry(&mut self, block_id: BlockId) {
        if let Some(block) = self.blocks.get_mut(&block_id) {
            block.is_entry = true;
        }
        self.entry_block = Some(block_id);
    }

    /// 标记出口块
    pub fn mark_exit(&mut self, block_id: BlockId) {
        if let Some(block) = self.blocks.get_mut(&block_id) {
            block.is_exit = true;
        }
        if !self.exit_blocks.contains(&block_id) {
            self.exit_blocks.push(block_id);
        }
    }

    /// 获取块数量
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// 获取边数量
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// 计算圈复杂度 (Cyclomatic Complexity)
    pub fn cyclomatic_complexity(&self) -> u32 {
        // CC = E - N + 2P
        // E = 边数, N = 节点数, P = 连通分量 (通常为1)
        let e = self.edges.len() as i32;
        let n = self.blocks.len() as i32;
        let p = 1; // 假设单连通分量
        
        (e - n + 2 * p).max(1) as u32
    }

    /// DFS 遍历所有可达块
    pub fn reachable_blocks(&self, start: BlockId) -> HashSet<BlockId> {
        let mut visited = HashSet::new();
        let mut stack = vec![start];
        
        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                continue;
            }
            
            visited.insert(current);
            
            if let Some(block) = self.blocks.get(&current) {
                for &succ in &block.successors {
                    if !visited.contains(&succ) {
                        stack.push(succ);
                    }
                }
            }
        }
        
        visited
    }
}

/// 支配树节点
#[derive(Debug, Clone)]
struct DominatorTreeNode {
    block_id: BlockId,
    children: Vec<Box<DominatorTreeNode>>,
}

/// 支配树
#[derive(Debug, Clone)]
pub struct DominatorTree {
    root: Option<Box<DominatorTreeNode>>,
    /// 直接支配关系: block -> immediate_dominator
    idom: HashMap<BlockId, Option<BlockId>>,
}

impl DominatorTree {
    /// 检查 block_a 是否支配 block_b
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        if a == b {
            return true;
        }

        let mut current = Some(b);
        while let Some(dom) = current.and_then(|id| self.idom.get(&id).copied()).flatten() {
            if dom == a {
                return true;
            }
            current = Some(dom);
        }

        false
    }
}

/// 循环信息
#[derive(Debug, Clone)]
pub struct LoopInfo {
    /// 循环头 (header)
    pub header: BlockId,
    
    /// 循环体中的所有块
    pub body: Vec<BlockId>,
    
    /// 回边 (back edge)
    pub back_edge: EdgeId,
    
    /// 循环类型
    pub loop_type: LoopType,
    
    /// 嵌套深度
    pub nesting_depth: usize,
}

/// 循环类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopType {
    /// While 循环
    WhileLoop,
    /// For 循环
    ForLoop,
    /// Do-While 循环
    DoWhileLoop,
    /// 无限循环
    InfiniteLoop,
    /// 其他 (递归等)
    Other,
}

/// 复杂度度量结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityMetrics {
    /// 圈复杂度
    pub cyclomatic_complexity: u32,
    
    /// 最大嵌套深度
    pub max_nesting_depth: usize,
    
    /// 总行数
    pub total_lines: usize,
    
    /// 函数数量
    pub function_count: usize,
    
    /// 平均函数长度 (行)
    pub avg_function_length: f64,
    
    /// 长函数列表 (>20 行)
    pub long_functions: Vec<LongFunctionInfo>,
}

/// 长函数信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongFunctionInfo {
    pub name: String,
    pub file_path: PathBuf,
    pub line_start: usize,
    pub line_end: usize,
    pub length: usize,
    pub complexity: u32,
}

/// 增强 Tree-sitter 解析器 (带 CFG 支持)
pub struct EnhancedTreeSitterParser {
    inner: TreeSitterParserManager,
}

impl EnhancedTreeSitterParser {
    /// 创建新的增强解析器
    pub fn new(config: ParserConfig) -> Self {
        Self {
            inner: TreeSitterParserManager::new(config),
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(ParserConfig::default())
    }

    /// 解析源码并构建 CFG
    pub async fn parse_with_cfg(
        &self,
        source: &str,
        language: LanguageId,
    ) -> Result<(ParseResult, ControlFlowGraph), ParseError> {
        // 首先进行标准解析
        let parse_result = self.inner.parse_source(source, language).await?;
        
        // 然后构建 CFG
        let cfg = self.build_cfg_for_function(&parse_result.root)?;
        
        Ok((parse_result, cfg))
    }

    /// 为指定函数构建 CFG
    fn build_cfg_for_function(
        &self,
        func_node: &AstNode,
    ) -> Result<ControlFlowGraph, ParseError> {
        let mut cfg = ControlFlowGraph::new();
        
        // 找到函数体
        let body = func_node.find_by_type(&NodeType::BlockStatement)
            .ok_or_else(|| ParseError::Internal("Function body not found".to_string()))?;
        
        // 创建入口块
        let entry = cfg.create_block();
        cfg.set_entry(entry);
        
        // 递归构建基本块
        self.build_cfg_recursive(body, &mut cfg, entry)?;
        
        // 标记出口块
        self.identify_exit_blocks(&mut cfg);
        
        // 检测循环
        cfg.loops = self.detect_loops(&cfg);
        
        // 构建支配树
        cfg.dominator_tree = Some(self.build_dominator_tree(&cfg));
        
        Ok(cfg)
    }

    /// 递归构建 CFG
    fn build_cfg_recursive(
        &self,
        node: &AstNode,
        cfg: &mut ControlFlowGraph,
        current_block: BlockId,
    ) -> Result<(), ParseError> {
        match &node.node_type {
            NodeType::IfStatement => {
                // if 语句: 创建两个分支
                let then_block = cfg.create_block();
                let else_block = cfg.create_block();
                let merge_block = cfg.create_block(); // 合并点
                
                // 当前块 -> then_block (真分支)
                cfg.add_edge(current_block, then_block, EdgeType::TrueBranch);
                
                // 当前块 -> else_block (假分支)
                cfg.add_edge(current_block, else_block, EdgeType::FalseBranch);
                
                // 处理 then 分支
                if let Some(then_body) = node.children.first() {
                    self.build_cfg_recursive(then_body, cfg, then_block)?;
                }
                cfg.add_edge(then_block, merge_block, EdgeType::Unconditional);
                
                // 处理 else 分支 (如果有)
                if node.children.len() > 1 {
                    if let Some(else_body) = node.children.get(1) {
                        self.build_cfg_recursive(else_body, cfg, else_block)?;
                    }
                } else {
                    // 没有 else，直接跳到合并点
                    cfg.add_edge(else_block, merge_block, EdgeType::Unconditional);
                }
                cfg.add_edge(else_block, merge_block, EdgeType::Unconditional);
                
                // 继续从合并点构建
                // 处理 if 后面的语句...
                for child in node.children.iter().skip(2) { // 跳过 then 和 else
                    self.build_cfg_recursive(child, cfg, merge_block)?;
                }
            }
            
            NodeType::ForStatement | NodeType::WhileStatement => {
                // 循环: 创建 loop header, body, exit
                let header_block = cfg.create_block();
                let body_block = cfg.create_block();
                let exit_block = cfg.create_block();
                
                // 当前块 -> header
                cfg.add_edge(current_block, header_block, EdgeType::Unconditional);
                
                // header -> body (条件为真时进入循环体)
                cfg.add_edge(header_block, body_block, EdgeType::TrueBranch);
                
                // header -> exit (条件为假时退出)
                cfg.add_edge(header_block, exit_block, EdgeType::FalseBranch);
                
                // 处理循环体
                if let Some(loop_body) = node.children.first() {
                    self.build_cfg_recursive(loop_body, cfg, body_block)?;
                }
                
                // body -> header (回边)
                cfg.add_edge(body_block, header_block, EdgeType::LoopBack);
                
                // 继续从 exit 构建
                for child in node.children.iter().skip(1) {
                    self.build_cfg_recursive(child, cfg, exit_block)?;
                }
            }
            
            NodeType::MatchStatement => {
                // match 语句: 创建多个 case 分支
                let merge_block = cfg.create_block();
                
                for (i, child) in node.children.iter().enumerate() {
                    let case_block = cfg.create_block();
                    
                    if i == 0 {
                        // 第一个 case
                        cfg.add_edge(current_block, case_block, EdgeType::CaseBranch(format!("case_{}", i)));
                    } else {
                        // fall-through 或新的 case (简化处理)
                        cfg.add_edge(current_block, case_block, EdgeType::CaseBranch(format!("case_{}", i)));
                    }
                    
                    self.build_cfg_recursive(child, cfg, case_block)?;
                    cfg.add_edge(case_block, merge_block, EdgeType::Unconditional);
                }
                
                // 继续从合并点构建
            }
            
            NodeType::ReturnStatement => {
                // 返回或 break: 连接到出口
                let exit = cfg.create_block();
                cfg.mark_exit(exit);
                cfg.add_edge(current_block, exit, EdgeType::Return);
            }
            
            _ => {
                // 其他语句: 保持在当前块中
                // 将当前节点的 ID 添加到当前块
                if let Some(block) = cfg.blocks.get_mut(&current_block) {
                    block.node_ids.push(node.id);
                }
                
                // 递归处理子节点
                for child in &node.children {
                    self.build_cfg_recursive(child, cfg, current_block)?;
                }
            }
        }
        
        Ok(())
    }

    /// 识别出口块
    fn identify_exit_blocks(&self, cfg: &mut ControlFlowGraph) {
        // 出口块特征:
        // 1. 没有后继的块
        // 2. 以 return/break 结尾的块

        let exit_blocks: Vec<BlockId> = cfg.blocks
            .iter()
            .filter(|(_, block)| block.successors.is_empty() && !block.is_entry)
            .map(|(id, _)| *id)
            .collect();

        for id in exit_blocks {
            cfg.mark_exit(id);
        }
    }

    /// 检测循环 (基于回边)
    fn detect_loops(&self, cfg: &ControlFlowGraph) -> Vec<LoopInfo> {
        let mut loops = Vec::new();
        
        // 找出所有回边 (目标块 ID < 源块 ID 的边通常表示回边)
        for edge in &cfg.edges {
            if edge.edge_type == EdgeType::LoopBack {
                let mut loop_body = cfg.reachable_blocks(edge.to);
                loop_body.remove(&edge.from); // 移除 header 本身
                
                loops.push(LoopInfo {
                    header: edge.to,
                    body: loop_body.into_iter().collect(),
                    back_edge: edge.id,
                    loop_type: LoopType::Other, // 需要更复杂的逻辑判断具体类型
                    nesting_depth: 0, // TODO: 计算嵌套深度
                });
            }
        }
        
        loops
    }

    /// 构建支配树 (简化的迭代算法)
    fn build_dominator_tree(&self, cfg: &ControlFlowGraph) -> DominatorTree {
        let mut dom = DominatorTree {
            root: None,
            idom: HashMap::new(),
        };
        
        let entry = match cfg.entry_block {
            Some(e) => e,
            None => return dom, // 无入口块则无法构建
        };
        
        // 初始化: 所有块的直接支配者设为入口块
        for &block_id in cfg.blocks.keys() {
            if block_id != entry {
                dom.idom.insert(block_id, Some(entry));
            } else {
                dom.idom.insert(block_id, None); // 入口块不被任何块支配
            }
        }
        
        // 迭代优化 (简化版: 仅做一次初始化)
        // 实际应使用 Lengauer-Tarjan 算法进行多次迭代直到收敛
        
        dom
    }

    /// 计算复杂度指标
    pub async fn calculate_metrics(
        &self,
        parse_result: &ParseResult,
    ) -> Result<ComplexityMetrics, ParseError> {
        let mut metrics = ComplexityMetrics {
            cyclomatic_complexity: 0,
            max_nesting_depth: 0,
            total_lines: parse_result.stats.source_lines,
            function_count: 0,
            avg_function_length: 0.0,
            long_functions: Vec::new(),
        };

        // 遍历所有函数
        let functions = parse_result.root.find_all_by_type(&NodeType::FunctionDeclaration);
        metrics.function_count = functions.len();

        for func_node in &functions {
            let location = &func_node.location;
            let length = (location.end_line - location.start_line + 1) as usize;
            
            // 为每个函数构建 CFG 并计算复杂度
            if let Ok(cfg) = self.build_cfg_for_function(func_node) {
                let cc = cfg.cyclomatic_complexity();
                metrics.cyclomatic_complexity += cc;
                
                // 记录长函数 (>20 行)
                if length > 20 {
                    metrics.long_functions.push(LongFunctionInfo {
                        name: func_node.name.clone().unwrap_or_else(|| "anonymous".to_string()),
                        file_path: PathBuf::new(), // TODO: 从 metadata 获取
                        line_start: location.start_line as usize,
                        line_end: location.end_line as usize,
                        length,
                        complexity: cc,
                    });
                }
            }
        }

        // 计算平均值
        if metrics.function_count > 0 {
            metrics.avg_function_length = 
                metrics.total_lines as f64 / metrics.function_count as f64;
        }

        Ok(metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parse_simple_function_with_cfg() {
        let parser = EnhancedTreeSitterParser::with_defaults();
        
        let code = r#"
fn example(x: i32) -> i32 {
    if x > 0 {
        x * 2
    } else {
        x + 1
    }
}
"#;

        let (result, cfg) = parser.parse_with_cfg(code, LanguageId::Rust).await.unwrap();
        
        assert!(cfg.block_count() >= 3); // 至少: entry, then, else+merge
        assert!(cfg.edge_count() >= 4); // 至少: entry->then, entry->else, then->merge, else->merge
        assert!(cfg.cyclomatic_complexity() >= 2); // if 语句至少贡献 1 个复杂度
    }

    #[test]
    fn test_cfg_basic_structure() {
        let mut cfg = ControlFlowGraph::new();
        
        let b0 = cfg.create_block(); // entry
        let b1 = cfg.create_block(); // body
        let b2 = cfg.create_block(); // exit
        
        cfg.set_entry(b0);
        cfg.mark_exit(b2);
        
        cfg.add_edge(b0, b1, EdgeType::Unconditional);
        cfg.add_edge(b1, b2, EdgeType::Return);
        
        assert_eq!(cfg.block_count(), 3);
        assert_eq!(cfg.edge_count(), 2);
        assert_eq!(cfg.entry_block, Some(b0));
        assert!(cfg.exit_blocks.contains(&b2));
        assert_eq!(cfg.cyclomatic_complexity(), 1); // 单路径: 1-2+2=1
    }

    #[test]
    fn test_reachable_blocks() {
        let mut cfg = ControlFlowGraph::new();
        
        let b0 = cfg.create_block();
        let b1 = cfg.create_block();
        let b2 = cfg.create_block();
        let b3 = cfg.create_block(); // 不可达
        
        cfg.set_entry(b0);
        cfg.add_edge(b0, b1, EdgeType::Unconditional);
        cfg.add_edge(b1, b2, EdgeType::Unconditional);
        // b3 没有任何边连接它
        
        let reachable = cfg.reachable_blocks(b0);
        
        assert!(reachable.contains(&b0));
        assert!(reachable.contains(&b1));
        assert!(reachable.contains(&b2));
        assert!(!reachable.contains(&b3)); // b3 不可达
    }

    #[tokio::test]
    async fn test_calculate_metrics() {
        let parser = EnhancedTreeSitterParser::with_defaults();
        
        let code = r#"
fn simple() { 1 }

fn complex(x: i32) -> i32 {
    if x > 0 {
        for i in 0..10 {
            if i % 2 == 0 {
                println!("{}", i);
            }
        }
    }
    x
}
"#;

        let result = parser.inner.parse_source(code, LanguageId::Rust).await.unwrap();
        let metrics = parser.calculate_metrics(&result).await.unwrap();
        
        assert_eq!(metrics.function_count, 2);
        assert!(metrics.total_lines > 0);
        assert!(metrics.cyclomatic_complexity > 0);
    }
}

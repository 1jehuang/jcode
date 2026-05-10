use serde::{Deserialize, Serialize};

pub mod cli {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct FindArgs {
        pub path: Option<String>,
        pub glob: Option<String>,
        pub paths_only: bool,
        pub debug_score: bool,
        pub query_parts: Vec<String>,
        pub file_type: Option<String>,
        pub json: bool,
        pub max_files: usize,
        pub hidden: bool,
        pub no_ignore: bool,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub enum FullRegionMode {
        File,
        Function,
        Class,
        Auto,
        Always,
        Never,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct GrepArgs {
        pub path: Option<String>,
        pub glob: Option<String>,
        pub paths_only: bool,
        pub query: String,
        pub regex: bool,
        pub file_type: Option<String>,
        pub json: bool,
        pub hidden: bool,
        pub no_ignore: bool,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct OutlineArgs {
        pub path: Option<String>,
        pub file: String,
        pub json: bool,
        pub max_items: Option<usize>,
        pub context_json: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SmartArgs {
        pub path: Option<String>,
        pub paths_only: bool,
        pub debug_score: bool,
        pub debug_plan: bool,
        pub terms: Vec<String>,
        pub json: bool,
        pub max_files: usize,
        pub max_regions: usize,
        pub full_region: FullRegionMode,
        pub file_type: Option<String>,
        pub glob: Option<String>,
        pub hidden: bool,
        pub no_ignore: bool,
        pub context_json: Option<String>,
    }
}

pub mod find {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct FindFile {
        pub path: String,
        pub matches: Vec<String>,
        pub role: String,
        pub why: Vec<String>,
        pub score: f64,
        pub structure: super::Structure,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct FindResult {
        pub files: Vec<FindFile>,
        pub total_matches: usize,
        pub total_files: usize,
        pub query: String,
    }

    pub fn run_find(
        _root: &PathBuf,
        _args: &super::cli::FindArgs,
    ) -> FindResult {
        FindResult {
            files: vec![],
            total_matches: 0,
            total_files: 0,
            query: String::new(),
        }
    }
}

pub mod outline {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct OutlineResult {
        pub symbols: Vec<String>,
        pub path: String,
        pub language: String,
        pub role: String,
        pub total_lines: usize,
        pub structure: super::Structure,
        pub context_applied: Option<String>,
    }

    pub fn run_outline(
        _root: &PathBuf,
        _args: &super::cli::OutlineArgs,
    ) -> anyhow::Result<OutlineResult> {
        Ok(OutlineResult {
            symbols: vec![],
            path: String::new(),
            language: String::new(),
            role: String::new(),
            total_lines: 0,
            structure: super::Structure::default(),
            context_applied: None,
        })
    }
}

pub mod search {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MatchLine {
        pub line_text: String,
        pub line_number: usize,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct MatchGroup {
        pub start_line: Option<usize>,
        pub end_line: Option<usize>,
        pub kind: String,
        pub label: String,
    }

    impl MatchGroup {
        pub fn resolved_matches<'a>(
            &self,
            matches: &'a [MatchLine],
        ) -> impl Iterator<Item = &'a MatchLine> {
            matches.iter()
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct OtherSymbol {
        pub kind: String,
        pub label: String,
        pub start_line: usize,
        pub end_line: usize,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct FileMatches {
        pub path: String,
        pub lines: Vec<String>,
        pub matches: Vec<MatchLine>,
        pub groups: Vec<MatchGroup>,
        pub total_symbols: usize,
        pub matched_symbol_count: usize,
        pub other_symbols: Vec<OtherSymbol>,
        pub other_symbols_omitted_count: usize,
        pub language: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct GrepResult {
        pub files: Vec<FileMatches>,
        pub total_matches: usize,
        pub total_files: usize,
        pub query: String,
    }

    pub fn run_grep(
        _root: &PathBuf,
        _args: &super::cli::GrepArgs,
    ) -> anyhow::Result<GrepResult> {
        Ok(GrepResult {
            files: vec![],
            total_matches: 0,
            total_files: 0,
            query: String::new(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Structure {
    pub items: Vec<StructureItem>,
    pub omitted_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureItem {
    pub kind: String,
    pub label: String,
    pub start_line: usize,
    pub end_line: usize,
    pub line_count: usize,
}

pub mod smart_dsl {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub enum Relation {
        Contains,
        Calls,
        Imports,
        Inherits,
        Rendered,
        CalledFrom,
        TriggeredFrom,
        Populated,
        ComesFrom,
        Handled,
        Defined,
        Implementation,
    }

    impl Relation {
        pub fn as_str(&self) -> &'static str {
            match self {
                Relation::Contains => "contains",
                Relation::Calls => "calls",
                Relation::Imports => "imports",
                Relation::Inherits => "inherits",
                Relation::Rendered => "rendered",
                Relation::CalledFrom => "called_from",
                Relation::TriggeredFrom => "triggered_from",
                Relation::Populated => "populated",
                Relation::ComesFrom => "comes_from",
                Relation::Handled => "handled",
                Relation::Defined => "defined",
                Relation::Implementation => "implementation",
            }
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SmartQuery {
        pub subject: String,
        pub relation: Relation,
        pub object: Option<String>,
        pub support: Vec<String>,
        pub kind: Option<String>,
        pub path_hint: Option<String>,
    }

    pub fn parse_smart_query(_query: &[String]) -> anyhow::Result<SmartQuery> {
        Ok(SmartQuery {
            subject: String::new(),
            relation: Relation::Contains,
            object: None,
            support: vec![],
            kind: None,
            path_hint: None,
        })
    }
}

pub mod smart_engine {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SmartFile {
        pub path: String,
        pub regions: Vec<SmartRegion>,
        pub role: String,
        pub why: Vec<String>,
        pub score: f64,
        pub structure: super::Structure,
        pub context_applied: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SmartRegion {
        pub name: String,
        pub kind: String,
        pub content: String,
        pub line_start: usize,
        pub line_end: usize,
        pub label: String,
        pub start_line: usize,
        pub end_line: usize,
        pub line_count: usize,
        pub score: f64,
        pub full_region: bool,
        pub body: String,
        pub why: Vec<String>,
        pub context_applied: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SmartResultSummary {
        pub total_files: usize,
        pub total_regions: usize,
        pub best_file: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SmartResult {
        pub files: Vec<SmartFile>,
        pub total_regions: usize,
        pub query: super::smart_dsl::SmartQuery,
        pub summary: SmartResultSummary,
    }

    pub fn run_smart(
        _root: &PathBuf,
        _query: &super::smart_dsl::SmartQuery,
        _args: &super::cli::SmartArgs,
    ) -> anyhow::Result<SmartResult> {
        Ok(SmartResult {
            files: vec![],
            total_regions: 0,
            query: super::smart_dsl::SmartQuery {
                subject: String::new(),
                relation: super::smart_dsl::Relation::Contains,
                object: None,
                support: vec![],
                kind: None,
                path_hint: None,
            },
            summary: SmartResultSummary {
                total_files: 0,
                total_regions: 0,
                best_file: None,
            },
        })
    }
}
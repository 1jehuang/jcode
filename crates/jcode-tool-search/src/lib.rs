//! # jcode-tool-search
//!
//! Deferred tool loading and search for reduced initial prompt token count.
//!
//! Instead of loading all tool definitions (including full descriptions) upfront
//! — which can consume thousands of tokens — this crate enables a two-phase
//! approach:
//!
//! 1. **Phase 1**: Load only tool *names* (compact strings). Parse them into
//!    searchable tokens via [`tool_name::ParsedToolName`].
//! 2. **Phase 2**: When the user (or agent) searches, use [`keyword_search`]
//!    to rank tools by relevance and lazily load descriptions only for matches
//!    via [`description_cache::DescriptionCache`].
//!
//! ## Tool naming conventions
//!
//! - **MCP tools** follow the pattern `mcp__<server>__<action>` (double-underscore
//!   delimited). These are parsed into [`tool_name::McpToolName`] with separate
//!   `server` and `action` fields.
//! - **Regular tools** use CamelCase names (e.g. `AgentGrep`, `WebSearch`).
//!
//! ## Scoring
//!
//! [`keyword_search::search_tools`] scores results as:
//!
//! | Match type       | Points |
//! |------------------|--------|
//! | Name exact       | 100    |
//! | Name contains    | 50     |
//! | Description      | 20     |
//! | Name partial     | 10     |

pub mod description_cache;
pub mod keyword_search;
pub mod tool_name;

// Re-export primary types at crate root for convenience.
pub use description_cache::{DescriptionCache, DescriptionState};
pub use keyword_search::{SearchResult, Tool, ToolEntry, rank_results, search_tools};
pub use tool_name::{ParsedToolName, parse_tool_name};

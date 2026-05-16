"""Batch-fix unused imports, dead_code, and other warnings in src/ directory."""
import re
import os

SRC = r'd:/studying/Codecargo/CarpAI/src'

# (file_suffix, line_search, full_line)
REMOVE_IMPORTS = [
    # tree_sitter.rs
    ('ast/tree_sitter.rs', 'use tree_sitter::{InputEdit, Language, Parser, Point, Tree};',
     'use tree_sitter::{InputEdit, Language, Parser, Point, Tree};'),
    # commands.rs
    ('cli/commands.rs', 'use std::collections::HashMap;', None),  # already removed by DAP split
    # p1_commands.rs
    ('cli/p1_commands.rs', 'use serde_json::json;', 'use serde_json::json;'),
    # mcp/dynamic_registry.rs
    ('mcp/dynamic_registry.rs', 'use anyhow::{Context, Result};', 'use anyhow::Result;'),
    # mcp/server.rs
    ('mcp/server.rs', 'RegisterResult, UnregisterResult', None),
    # server/lsp_event_bridge.rs
    ('server/lsp_event_bridge.rs', 'use tracing::{info, warn};', None),
    # server/conflict_detector.rs
    ('server/conflict_detector.rs', 'use tracing::{debug, info};', None),
    # server/collab.rs
    ('server/collab.rs', 'use tokio::sync::{mpsc, broadcast, RwLock};',
     'use tokio::sync::{broadcast, RwLock};'),
    # session/sharing.rs
    ('session/sharing.rs', 'use super::replay::{RecordedEvent, RecordedSession, SessionMetadata};',
     'use super::replay::{RecordedEvent, RecordedSession};'),
    # tui/ui_context_actions.rs
    ('tui/ui_context_actions.rs', 'use crate::tui::ui_blocks::{CommandBlock, ActionType, BlockType};',
     'use crate::tui::ui_blocks::{CommandBlock, ActionType};'),
    # auto_mode/aho_corasick.rs
    ('auto_mode/aho_corasick.rs', 'use std::collections::{HashMap, HashSet};', None),
    # auto_mode/enhanced_confidence.rs
    ('auto_mode/enhanced_confidence.rs', 'use crate::auto_mode::engine::ActionType;', None),
    # auto_mode/safety.rs
    ('auto_mode/safety.rs', 'use std::cmp::Ordering;', None),
    # refactor_engine.rs
    ('refactor_engine.rs', 'use crate::refactor::PreciseEditEngine;', None),
    # completion/bash/completer.rs
    ('completion/bash/completer.rs', 'use registry::CommandCategory;',
     None),
    # ai_enhanced/mod.rs
    ('ai_enhanced/mod.rs', 'use anyhow::Result;', None),
    # ssh/config.rs
    ('ssh/config.rs', 'use std::path::Path;', None),
    # ssh/tunnel.rs
    ('ssh/tunnel.rs', 'use std::process::{Command, Stdio};', 'use std::process::Command;'),
    # ssh/tunnel.rs duration
    ('ssh/tunnel.rs', 'use std::time::Duration;', None),
    # ssh/resilience.rs
    ('ssh/resilience.rs', 'use std::sync::Arc;', None),
    # ssh/resilience.rs SessionState
    ('ssh/resilience.rs', 'SessionState', None),
    # ssh/sftp.rs
    ('ssh/sftp.rs', 'BufReader', None),
    # ssh/agent.rs
    ('ssh/agent.rs', 'BufReader', None),
    # ssh/transfer.rs
    ('ssh/transfer.rs', 'use std::sync::{Arc, Mutex};', None),
]

def fix_file_unused_import(file_rel):
    filepath = os.path.join(SRC, file_rel)
    if not os.path.exists(filepath):
        print(f"  SKIP (not found): {file_rel}")
        return False
    
    with open(filepath, 'r', encoding='utf-8') as f:
        lines = f.readlines()
    
    original = lines[:]
    new_lines = []
    removed = False
    
    for line in lines:
        keep = True
        for search, full in REMOVE_IMPORTS:
            if file_rel == search:
                if full and line.strip() == full:
                    keep = False
                    removed = True
                    break
                elif search == 'ast/tree_sitter.rs' and 'Range' in line and 'use tree_sitter' in line:
                    # Remove Range from tree_sitter import
                    line = line.replace('Range, ', '').replace(', Range', '').replace('Range', '')
                    if line.strip().endswith(', {'):
                        continue  # empty braces
                    removed = True
                elif search == 'refactor_engine.rs':
                    if 'CoordinationResult' in line or 'PreciseEditEngine' in line or 'Context' in line:
                        keep = False
                        removed = True
                        break
        if keep:
            new_lines.append(line)
    
    if original != new_lines:
        with open(filepath, 'w', encoding='utf-8') as f:
            f.writelines(new_lines)
        return True
    return removed

fixed = 0
for file_rel, _, _ in REMOVE_IMPORTS:
    if fix_file_unused_import(file_rel):
        fixed += 1

print(f"Fixed {fixed} files with unused import removals.")

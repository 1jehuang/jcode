# CarpAI Neovim Plugin

AI-powered coding assistant for Neovim with inline completion, chat, code review, and refactoring.

## Requirements

- Neovim 0.9+ (0.10+ for inline ghost text)
- `curl` and `jq` installed
- CarpAI server running (`jcode` or `carpai-server`)

## Installation

### lazy.nvim
```lua
{
    "carpai/carpai-nvim",
    config = function()
        require("carpai").setup({
            server_url = "http://localhost:8080",
            -- Optional API key
            api_key = "",
        })
    end,
}
```

### packer.nvim
```lua
use {
    "carpai/carpai-nvim",
    config = function()
        require("carpai").setup({})
    end,
}
```

## Configuration

### Default settings
```lua
require("carpai").setup({
    server_url = "http://localhost:8080",
    api_key = "",
    completion_enabled = true,
    chat_enabled = true,
    keymaps = {
        toggle_chat = "<C-S-c>",    -- Ctrl+Shift+C to toggle chat
        accept_completion = "<Tab>",
        dismiss_completion = "<C-e>",
        explain = "<Leader>ae",     -- Explain selected code
        review = "<Leader>ar",      -- Review current file
        refactor = "<Leader>at",    -- Refactor selected code
        quick_fix = "<Leader>af",   -- Quick fix diagnostics
    },
    completion = {
        debounce_ms = 150,
        max_lines = 200,
        context_lines = 50,
    },
})
```

## Commands

| Command | Description |
|---------|-------------|
| `:CarpAIHealth` | Check server connection |
| `:CarpAIReview` | Review current buffer |
| `:CarpAIExplain` | Explain selected code |
| `:CarpAIExplain <range>` | Explain range |
| `:CarpAIRefactor` | Refactor selected code |
| `:CarpAIChat` | Open chat panel |

## Features

- **Inline Completion**: Ghost text suggestions while typing (Neovim 0.10+)
- **Chat Panel**: Side panel for conversational AI assistance
- **Code Review**: AI-powered code analysis with diagnostics
- **Code Explanation**: Explain selected code in natural language
- **Code Refactoring**: AI-assisted code transformation
- **Quick Fix**: Automatic fix for diagnostics

## MCP Integration

The plugin supports MCP (Model Context Protocol) for tool integration:

```lua
-- In your Neovim config
require("carpai").setup({
    mcp = {
        auto_connect = true,
        servers = {
            ["github"] = {
                command = "python",
                args = {"mcp-servers/github/src/server.py"},
            },
        },
    },
})
```

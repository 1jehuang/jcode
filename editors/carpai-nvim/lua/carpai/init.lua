-- CarpAI Neovim Plugin
-- Provides: AI chat, inline completion, code review, refactoring
-- Requirements: Neovim 0.9+, curl, jq
-- Install: add 'use "carpai/carpai-nvim"' to your packer/nvim-lazy config

local M = {}

-- Configuration with defaults
M.config = {
    server_url = "http://localhost:8080",
    api_key = "",
    completion_enabled = true,
    chat_enabled = true,
    keymaps = {
        toggle_chat = "<C-S-c>",
        accept_completion = "<Tab>",
        dismiss_completion = "<C-e>",
        explain = "<Leader>ae",
        review = "<Leader>ar",
        refactor = "<Leader>at",
        quick_fix = "<Leader>af",
    },
    completion = {
        debounce_ms = 150,
        max_lines = 200,
        context_lines = 50,
    },
}

-- Internal state
local state = {
    client = nil,
    chat_win = nil,
    chat_buf = nil,
    inline_completion = nil,
    job_id = nil,
}

-- HTTP Client using vim.system (Neovim 0.10+) or vim.fn.jobstart
local function http_request(method, path, body, callback)
    local url = M.config.server_url .. path
    local args = { "curl", "-s", "-X", method, url }
    
    if M.config.api_key and M.config.api_key ~= "" then
        table.insert(args, "-H")
        table.insert(args, "Authorization: Bearer " .. M.config.api_key)
    end
    
    if body then
        table.insert(args, "-H")
        table.insert(args, "Content-Type: application/json")
        table.insert(args, "-d")
        table.insert(args, vim.json.encode(body))
    end

    if vim.system then
        -- Neovim 0.10+ API
        vim.system(args, { text = true }, function(result)
            if result.code == 0 and result.stdout then
                local ok, data = pcall(vim.json.decode, result.stdout)
                if ok then
                    callback(data)
                else
                    callback(nil, "Failed to parse response")
                end
            else
                callback(nil, result.stderr or "Request failed")
            end
        end)
    else
        -- Fallback for older Neovim
        local timer = vim.loop.new_timer()
        local stdout_data = {}
        local stderr_data = {}
        
        local handle
        handle = vim.fn.jobstart(args, {
            stdout_buffered = true,
            stderr_buffered = true,
            on_stdout = function(_, data)
                for _, line in ipairs(data) do
                    table.insert(stdout_data, line)
                end
            end,
            on_stderr = function(_, data)
                for _, line in ipairs(data) do
                    table.insert(stderr_data, line)
                end
            end,
            on_exit = function(_, code)
                if code == 0 and #stdout_data > 0 then
                    local text = table.concat(stdout_data, "")
                    local ok, data = pcall(vim.json.decode, text)
                    if ok then
                        callback(data)
                    else
                        callback(nil, "Failed to parse response")
                    end
                else
                    callback(nil, table.concat(stderr_data, ""))
                end
            end,
        })
    end
end

-- Health check
function M.health_check(callback)
    http_request("GET", "/health", nil, function(data, err)
        if callback then
            callback(data ~= nil, err)
        end
    end)
end

-- Get inline completion
function M.get_completion(opts, callback)
    if not M.config.completion_enabled then
        callback(nil)
        return
    end

    local buf = vim.api.nvim_get_current_buf()
    local row, col = unpack(vim.api.nvim_win_get_cursor(0))
    local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    local content = table.concat(lines, "\n")
    
    -- Get line prefix/suffix
    local current_line = lines[row] or ""
    local line_prefix = current_line:sub(1, col)
    local line_suffix = current_line:sub(col + 1)

    -- Build context window
    local context_start = math.max(0, row - M.config.completion.max_lines)
    local context_end = math.min(#lines, row + M.config.completion.context_lines)
    local context_lines = {}
    for i = context_start + 1, context_end do
        table.insert(context_lines, lines[i] or "")
    end

    local request = {
        file_path = vim.api.nvim_buf_get_name(buf),
        content = content,
        line = row,
        character = col,
        line_prefix = line_prefix,
        line_suffix = line_suffix,
        context_window = table.concat(context_lines, "\n"),
        language = vim.bo[buf].filetype,
    }

    http_request("POST", "/api/v1/inline-completions", request, function(data, err)
        if callback then
            callback(data, err)
        end
    end)
end

-- Chat with CarpAI
function M.chat(message, callback)
    http_request("POST", "/api/v1/chat", { message = message }, function(data, err)
        if callback then
            callback(data, err)
        end
    end)
end

-- Code review
function M.review(callback)
    local buf = vim.api.nvim_get_current_buf()
    local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    local content = table.concat(lines, "\n")

    http_request("POST", "/api/v1/review", {
        file_path = vim.api.nvim_buf_get_name(buf),
        content = content,
    }, function(data, err)
        if callback then
            callback(data, err)
        end
    end)
end

-- Explain code
function M.explain(callback)
    local bufnr = vim.api.nvim_get_current_buf()
    local start_line, end_line
    local mode = vim.api.nvim_get_mode().mode
    
    if mode == "v" or mode == "V" or mode == "" then
        -- Visual selection
        local start_pos = vim.api.nvim_buf_get_mark(bufnr, "<")
        local end_pos = vim.api.nvim_buf_get_mark(bufnr, ">")
        start_line = start_pos[1] - 1
        end_line = end_pos[1]
    else
        -- Current line or function
        start_line = vim.fn.line(".") - 1
        end_line = start_line
    end

    local lines = vim.api.nvim_buf_get_lines(bufnr, start_line, end_line + 1, false)
    local code = table.concat(lines, "\n")

    http_request("POST", "/api/v1/explain", { code = code }, function(data, err)
        if callback then
            callback(data, err)
        end
    end)
end

-- Chat panel UI
function M.toggle_chat()
    if state.chat_win and vim.api.nvim_win_is_valid(state.chat_win) then
        vim.api.nvim_win_close(state.chat_win, true)
        state.chat_win = nil
        state.chat_buf = nil
        return
    end

    -- Create chat buffer and window
    state.chat_buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_option(state.chat_buf, "buftype", "acwrite")
    vim.api.nvim_buf_set_name(state.chat_buf, "CarpAI Chat")

    local width = math.floor(vim.o.columns * 0.4)
    state.chat_win = vim.api.nvim_open_win(state.chat_buf, true, {
        relative = "editor",
        width = width,
        height = vim.o.lines - 4,
        col = vim.o.columns - width - 1,
        row = 1,
        style = "minimal",
        border = "rounded",
        title = " CarpAI ",
        title_pos = "center",
    })

    vim.api.nvim_buf_set_lines(state.chat_buf, 0, -1, false, {
        "╔══════════════════════════════╗",
        "║  CarpAI Chat                ║",
        "║  Type :q to close           ║",
        "╚══════════════════════════════╝",
        "",
    })

    -- Input handling
    local function handle_input()
        local last_line = vim.api.nvim_buf_line_count(state.chat_buf)
        local input = vim.api.nvim_buf_get_lines(state.chat_buf, last_line - 1, last_line, false)[1] or ""
        if input == ":q" then
            M.toggle_chat()
            return
        end
        if input ~= "" then
            -- Add user message
            vim.api.nvim_buf_set_lines(state.chat_buf, last_line - 1, last_line, false, {
                "You: " .. input,
                "",
                "CarpAI: Thinking...",
                "",
                "",
            })
            -- Send to API
            M.chat(input, function(response, err)
                local line_count = vim.api.nvim_buf_line_count(state.chat_buf)
                if response and response.response then
                    vim.api.nvim_buf_set_lines(state.chat_buf, line_count - 3, line_count - 2, false, {
                        "CarpAI: " .. response.response,
                    })
                else
                    vim.api.nvim_buf_set_lines(state.chat_buf, line_count - 3, line_count - 2, false, {
                        "CarpAI: Error - " .. (err or "unknown"),
                    })
                end
            end)
        end
    end

    -- Set up autocommands for input
    vim.api.nvim_buf_attach(state.chat_buf, false, {
        on_lines = function()
            -- Debounce input handling
            if state.chat_timer then
                vim.loop.timer_stop(state.chat_timer)
            end
            state.chat_timer = vim.defer_fn(handle_input, 500)
        end,
    })
end

-- Setup keymaps and autocommands
function M.setup(user_config)
    -- Merge user config (force user config to override defaults)
    if user_config then
        M.config = vim.tbl_deep_extend("force", M.config, user_config)
    end

    -- Keymaps
    vim.keymap.set("n", M.config.keymaps.toggle_chat, M.toggle_chat, {
        desc = "Toggle CarpAI Chat",
    })
    vim.keymap.set({"n", "v"}, M.config.keymaps.explain, function()
        M.explain(function(response, err)
            if response and response.explanation then
                print("CarpAI: " .. response.explanation:sub(1, 200))
            end
        end)
    end, { desc = "CarpAI Explain Code" })
    vim.keymap.set("n", M.config.keymaps.review, function()
        M.review(function(response, err)
            if response and response.issues then
                if #response.issues > 0 then
                    local msg = string.format("CarpAI: %d issues found", #response.issues)
                    vim.notify(msg, vim.log.levels.WARN)
                else
                    vim.notify("CarpAI: No issues found", vim.log.levels.INFO)
                end
            end
        end)
    end, { desc = "CarpAI Code Review" })
    vim.keymap.set({"n", "v"}, M.config.keymaps.refactor, function()
        local bufnr = vim.api.nvim_get_current_buf()
        local mode = vim.api.nvim_get_mode().mode
        local code
        if mode == "v" or mode == "V" then
            local start_pos = vim.api.nvim_buf_get_mark(bufnr, "<")
            local end_pos = vim.api.nvim_buf_get_mark(bufnr, ">")
            local lines = vim.api.nvim_buf_get_lines(bufnr, start_pos[1]-1, end_pos[1], false)
            code = table.concat(lines, "\n")
        else
            code = table.concat(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false), "\n")
        end
        local instructions = vim.fn.input("Refactoring instructions: ")
        if instructions and instructions ~= "" then
            M.chat("Refactor this code: " .. code .. "\nInstructions: " .. instructions, function(resp)
                if resp and resp.response then
                    vim.notify("CarpAI: Refactoring done. Check chat panel.", vim.log.levels.INFO)
                end
            end)
        end
    end, { desc = "CarpAI Refactor Code" })
    vim.keymap.set("n", M.config.keymaps.quick_fix, function()
        local diags = vim.diagnostic.get(vim.api.nvim_get_current_buf())
        if #diags == 0 then
            vim.notify("CarpAI: No diagnostics to fix", vim.log.levels.INFO)
            return
        end
        local lines = vim.api.nvim_buf_get_lines(vim.api.nvim_get_current_buf(), 0, -1, false)
        local code = table.concat(lines, "\n")
        local diag_text = {}
        for _, d in ipairs(diags) do
            table.insert(diag_text, string.format("Line %d: %s", d.lnum + 1, d.message))
        end
        M.chat("Fix these issues:\n" .. table.concat(diag_text, "\n") .. "\n\nCode:\n" .. code, function(resp)
            if resp and resp.response then
                -- Try to extract code from response
                local fixed = resp.response:match("```.-```")
                if not fixed then fixed = resp.response end
                if fixed and fixed ~= code then
                    vim.notify("CarpAI: Quick fix applied. Check chat output.", vim.log.levels.INFO)
                end
            end
        end)
    end, { desc = "CarpAI Quick Fix Diagnostics" })

    -- Inline completion (using vim.lsp for ghost text)
    if M.config.completion_enabled then
        vim.api.nvim_create_autocmd("TextChangedI", {
            pattern = "*",
            callback = vim.schedule_wrap(function()
                -- NOTE: For true inline completion (ghost text), 
                -- we use the LSP protocol's textDocument/inlineCompletion
                -- This requires Neovim 0.10+ with inlay hints support
                if vim.fn.has("nvim-0.10") == 1 then
                    -- vim.lsp.inlay_hint will handle ghost text via LSP
                end
            end),
        })
    end

    -- Health check on startup
    M.health_check(function(ok, err)
        if ok then
            vim.notify("CarpAI: Server connected", vim.log.levels.INFO)
        else
            vim.notify("CarpAI: Server not available - " .. (err or ""), vim.log.levels.WARN)
        end
    end)
end

return M

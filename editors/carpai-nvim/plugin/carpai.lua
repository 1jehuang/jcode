-- CarpAI Neovim Plugin - Plugin Entry Point
-- This file is auto-loaded by Neovim's plugin loader

if vim.g.loaded_carpai then
    return
end
vim.g.loaded_carpai = true

local carpai = require("carpai")

-- User commands
vim.api.nvim_create_user_command("CarpAIHealth", function()
    carpai.health_check(function(ok, err)
        if ok then
            vim.notify("CarpAI: Server connected ✓", vim.log.levels.INFO)
        else
            vim.notify("CarpAI: Server not available - " .. (err or "unknown"), vim.log.levels.WARN)
        end
    end)
end, {})

vim.api.nvim_create_user_command("CarpAIReview", function()
    carpai.review(function(response)
        if response and response.issues then
            if #response.issues == 0 then
                vim.notify("CarpAI: No issues found", vim.log.levels.INFO)
                return
            end
            -- Add diagnostics
            local diagnostic_items = {}
            for _, issue in ipairs(response.issues) do
                table.insert(diagnostic_items, {
                    lnum = issue.line or 0,
                    col = issue.column or 0,
                    severity = issue.severity == "error" and vim.diagnostic.severity.ERROR
                        or issue.severity == "warning" and vim.diagnostic.severity.WARN
                        or vim.diagnostic.severity.INFO,
                    message = issue.message,
                    source = "CarpAI",
                })
            end
            vim.diagnostic.set(vim.api.nvim_get_current_buf(), diagnostic_items)
            vim.notify(string.format("CarpAI: %d issues found", #response.issues), vim.log.levels.WARN)
        end
    end)
end, {})

vim.api.nvim_create_user_command("CarpAIExplain", function(opts)
    carpai.explain(function(response)
        if response and response.explanation then
            -- Open explanation in a new buffer or floating window
            local buf = vim.api.nvim_create_buf(false, true)
            vim.api.nvim_buf_set_lines(buf, 0, -1, false, vim.split(response.explanation, "\n"))
            vim.api.nvim_buf_set_option(buf, "buftype", "nofile")
            vim.api.nvim_buf_set_name(buf, "CarpAI Explanation")
            vim.api.nvim_set_current_buf(buf)
        end
    end)
end, { range = true })

vim.api.nvim_create_user_command("CarpAIChat", function()
    carpai.toggle_chat()
end, {})

vim.api.nvim_create_user_command("CarpAIRefactor", function(opts)
    local range = opts.range
    local bufnr = vim.api.nvim_get_current_buf()
    local code
    if range > 0 then
        local lines = vim.api.nvim_buf_get_lines(bufnr, opts.line1 - 1, opts.line2, false)
        code = table.concat(lines, "\n")
    else
        code = table.concat(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false), "\n")
    end
    local instructions = vim.fn.input("Refactoring instructions: ")
    if instructions and instructions ~= "" then
        carpai.chat("Refactor: " .. code .. "\nInstructions: " .. instructions, function(resp)
            if resp and resp.response then
                vim.notify("CarpAI: Refactoring complete ✓", vim.log.levels.INFO)
            else
                vim.notify("CarpAI: Refactoring failed", vim.log.levels.ERROR)
            end
        end)
    end
end, { range = true })

vim.api.nvim_create_user_command("CarpAIFix", function()
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
    carpai.chat("Fix issues:\n" .. table.concat(diag_text, "\n") .. "\n\nCode:\n" .. code, function(resp)
        if resp and resp.response then
            vim.notify("CarpAI: Fix suggestion received ✓", vim.log.levels.INFO)
        end
    end)
end, {})

vim.api.nvim_create_user_command("CarpAITestGen", function()
    local bufnr = vim.api.nvim_get_current_buf()
    local code = table.concat(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false), "\n")
    carpai.chat("Generate tests for this code:\n" .. code, function(resp)
        if resp and resp.response then
            -- Open a new buffer with the test code
            local buf = vim.api.nvim_create_buf(false, true)
            vim.api.nvim_buf_set_lines(buf, 0, -1, false, vim.split(resp.response, "\n"))
            vim.api.nvim_set_current_buf(buf)
            vim.notify("CarpAI: Tests generated ✓", vim.log.levels.INFO)
        end
    end)
end, {})

-- Auto-setup with defaults if not configured by user
if not vim.g.carpai_setup_done then
    pcall(function()
        local ok, _ = pcall(require, "carpai")
        if ok then
            -- User should call setup() explicitly; 
            -- this ensures the plugin loads without config
        end
    end)
end

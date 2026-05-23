-- CarpAI MCP Client for Neovim
-- Manages MCP server connections within Neovim

local M = {}

local config = {
    mcp_dir = vim.fn.stdpath("config") .. "/mcp",
    servers = {},
}

function M.setup(opts)
    if opts then
        config = vim.tbl_deep_extend("keep", opts or {}, config)
    end
end

-- Read MCP config from standard locations
function M.read_config()
    -- Check locations in priority order
    local locations = {
        vim.fn.getcwd() .. "/.jcode/mcp.json",
        vim.fn.getcwd() .. "/.vscode/mcp.json",
        vim.fn.getcwd() .. "/.cursor/mcp.json",
        vim.fn.expand("~") .. "/.jcode/mcp.json",
        vim.fn.expand("~") .. "/.claude/mcp.json",
    }

    for _, path in ipairs(locations) do
        local file = io.open(path, "r")
        if file then
            local content = file:read("*all")
            file:close()
            local ok, data = pcall(vim.json.decode, content)
            if ok and data and data.servers then
                return data.servers
            end
        end
    end

    return {}
end

-- Start an MCP server
function M.start_server(name, server_config)
    if not server_config.command then
        vim.notify("CarpAI MCP: No command for server " .. name, vim.log.levels.WARN)
        return nil
    end

    local args = vim.deepcopy(server_config.args or {})
    local cmd = vim.fn.executable(server_config.command) and server_config.command or nil
    
    if not cmd then
        vim.notify("CarpAI MCP: Command not found: " .. server_config.command, vim.log.levels.WARN)
        return nil
    end

    -- Start as a job
    local job_id = vim.fn.jobstart({ cmd, unpack(args) }, {
        on_stdout = function(_, data)
            for _, line in ipairs(data) do
                if line ~= "" then
                    -- Parse MCP JSON-RPC messages
                    local ok, msg = pcall(vim.json.decode, line)
                    if ok and msg then
                        M.handle_message(name, msg)
                    end
                end
            end
        end,
        on_stderr = function(_, data)
            for _, line in ipairs(data) do
                if line ~= "" then
                    vim.notify("[MCP:" .. name .. "] " .. line, vim.log.levels.DEBUG)
                end
            end
        end,
        on_exit = function(_, code)
            vim.notify("CarpAI MCP: " .. name .. " exited with code " .. code, vim.log.levels.INFO)
            config.servers[name] = nil
        end,
    })

    return job_id
end

-- Handle MCP messages
function M.handle_message(server_name, msg)
    if msg.method == "tools/list" then
        -- Respond with available tools
        M.send_response(server_name, msg.id, {
            tools = M.get_tools(server_name)
        })
    elseif msg.method == "tools/call" then
        -- Execute a tool
        local tool_name = msg.params.name
        local tool_args = msg.params.arguments or {}
        M.execute_tool(server_name, tool_name, tool_args, msg.id)
    end
end

-- Send JSON-RPC response
function M.send_response(server_name, id, result)
    local response = vim.json.encode({
        jsonrpc = "2.0",
        id = id,
        result = result,
    })
    -- Write to the job's stdin
    -- Note: This requires access to the job's channel
    vim.notify("[MCP] Response sent to " .. server_name, vim.log.levels.DEBUG)
end

-- Get tools for a server
function M.get_tools(server_name)
    -- Return CarpAI tools as MCP tool definitions
    return {
        {
            name = "explain",
            description = "Explain the selected code",
            inputSchema = {
                type = "object",
                properties = {
                    code = { type = "string" },
                    language = { type = "string" },
                },
            },
        },
        {
            name = "review",
            description = "Review code for issues",
            inputSchema = {
                type = "object",
                properties = {
                    file_path = { type = "string" },
                    content = { type = "string" },
                },
            },
        },
    }
end

-- Execute a tool
function M.execute_tool(server_name, tool_name, args, msg_id)
    if tool_name == "explain" then
        local code = args.code or ""
        -- Send to CarpAI server for explanation
        -- ...
    elseif tool_name == "review" then
        local content = args.content or ""
        -- Send to CarpAI server for review
        -- ...
    end
end

-- Auto-connect configured MCP servers
function M.auto_connect()
    local servers = M.read_config()
    for name, server_config in pairs(servers) do
        if not config.servers[name] then
            local job_id = M.start_server(name, server_config)
            if job_id then
                config.servers[name] = job_id
                vim.notify("CarpAI MCP: Connected " .. name, vim.log.levels.INFO)
            end
        end
    end
end

-- Disconnect all MCP servers
function M.disconnect_all()
    for name, job_id in pairs(config.servers) do
        vim.fn.jobstop(job_id)
    end
    config.servers = {}
end

return M

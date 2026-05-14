use std::collections::HashMap;

pub fn get_translations(locale_code: &str) -> HashMap<String, String> {
    match locale_code {
        "zh-CN" => chinese_simplified(),
        "zh-TW" => chinese_traditional(),
        "ja" => japanese(),
        "ko" => korean(),
        _ => english(),
    }
}

fn english() -> HashMap<String, String> {
    let mut t = HashMap::new();

    // App
    t.insert("app.name".to_string(), "CarpAI".to_string());
    t.insert("app.tagline".to_string(), "Your AI-Powered Development Assistant".to_string());

    // Common
    t.insert("common.ok".to_string(), "OK".to_string());
    t.insert("common.cancel".to_string(), "Cancel".to_string());
    t.insert("common.save".to_string(), "Save".to_string());
    t.insert("common.delete".to_string(), "Delete".to_string());
    t.insert("common.edit".to_string(), "Edit".to_string());
    t.insert("common.search".to_string(), "Search".to_string());
    t.insert("common.loading".to_string(), "Loading...".to_string());
    t.insert("common.error".to_string(), "Error".to_string());
    t.insert("common.success".to_string(), "Success".to_string());

    // Navigation
    t.insert("nav.dashboard".to_string(), "Dashboard".to_string());
    t.insert("nav.tasks".to_string(), "Tasks".to_string());
    t.insert("nav.plugins".to_string(), "Plugins".to_string());
    t.insert("nav.settings".to_string(), "Settings".to_string());
    t.insert("nav.help".to_string(), "Help".to_string());

    // Tasks
    t.insert("tasks.title".to_string(), "Tasks".to_string());
    t.insert("tasks.create".to_string(), "Create Task".to_string());
    t.insert("tasks.list".to_string(), "Task List".to_string());
    t.insert("tasks.status.todo".to_string(), "To Do".to_string());
    t.insert("tasks.status.in_progress".to_string(), "In Progress".to_string());
    t.insert("tasks.status.done".to_string(), "Done".to_string());
    t.insert("tasks.status.cancelled".to_string(), "Cancelled".to_string());
    t.insert("tasks.priority.low".to_string(), "Low".to_string());
    t.insert("tasks.priority.medium".to_string(), "Medium".to_string());
    t.insert("tasks.priority.high".to_string(), "High".to_string());
    t.insert("tasks.priority.critical".to_string(), "Critical".to_string());
    t.insert("tasks.one".to_string(), "{0} task".to_string());
    t.insert("tasks.other".to_string(), "{0} tasks".to_string());

    // Plugins
    t.insert("plugins.title".to_string(), "Plugins".to_string());
    t.insert("plugins.install".to_string(), "Install Plugin".to_string());
    t.insert("plugins.uninstall".to_string(), "Uninstall".to_string());
    t.insert("plugins.enabled".to_string(), "Enabled".to_string());
    t.insert("plugins.disabled".to_string(), "Disabled".to_string());
    t.insert("plugins.marketplace".to_string(), "Plugin Marketplace".to_string());
    t.insert("plugins.search_placeholder".to_string(), "Search plugins...".to_string());

    // Dashboard
    t.insert("dashboard.title".to_string(), "Dashboard".to_string());
    t.insert("dashboard.system_status".to_string(), "System Status".to_string());
    t.insert("dashboard.cpu_usage".to_string(), "CPU Usage".to_string());
    t.insert("dashboard.memory_usage".to_string(), "Memory Usage".to_string());
    t.insert("dashboard.disk_usage".to_string(), "Disk Usage".to_string());
    t.insert("dashboard.active_sessions".to_string(), "Active Sessions".to_string());
    t.insert("dashboard.performance".to_string(), "Performance".to_string());

    // SSH
    t.insert("ssh.title".to_string(), "SSH Remote".to_string());
    t.insert("ssh.connect".to_string(), "Connect".to_string());
    t.insert("ssh.disconnect".to_string(), "Disconnect".to_string());
    t.insert("ssh.execute".to_string(), "Execute Command".to_string());
    t.insert("ssh.upload".to_string(), "Upload File".to_string());
    t.insert("ssh.download".to_string(), "Download File".to_string());
    t.insert("ssh.connected".to_string(), "Connected".to_string());
    t.insert("ssh.disconnected".to_string(), "Disconnected".to_string());

    // Plan Mode
    t.insert("plan_mode.title".to_string(), "Plan Mode".to_string());
    t.insert("plan_mode.enter".to_string(), "Enter Plan Mode".to_string());
    t.insert("plan_mode.exit".to_string(), "Exit Plan Mode".to_string());
    t.insert("plan_mode.step_pending".to_string(), "Pending".to_string());
    t.insert("plan_mode.step_approved".to_string(), "Approved".to_string());
    t.insert("plan_mode.step_completed".to_string(), "Completed".to_string());
    t.insert("plan_mode.step_rejected".to_string(), "Rejected".to_string());

    // Auto Mode
    t.insert("auto_mode.title".to_string(), "Auto Mode".to_string());
    t.insert("auto_mode.enabled".to_string(), "Auto Mode Enabled".to_string());
    t.insert("auto_mode.disabled".to_string(), "Auto Mode Disabled".to_string());
    t.insert("auto_mode.auto_approve".to_string(), "Auto-Approved".to_string());
    t.insert("auto_mode.requires_confirmation".to_string(), "Requires Confirmation".to_string());
    t.insert("auto_mode.manual_review".to_string(), "Manual Review Required".to_string());

    // Version Manager
    t.insert("version.current".to_string(), "Current Version".to_string());
    t.insert("version.install".to_string(), "Install Version".to_string());
    t.insert("version.rollback".to_string(), "Rollback".to_string());
    t.insert("version.changelog".to_string(), "Changelog".to_string());
    t.insert("version.rollback_point".to_string(), "Rollback Point".to_string());

    // Session Export
    t.insert("export.title".to_string(), "Export Session".to_string());
    t.insert("export.format_json".to_string(), "JSON Format".to_string());
    t.insert("export.format_markdown".to_string(), "Markdown Format".to_string());
    t.insert("export.success".to_string(), "Session exported successfully".to_string());

    // Errors
    t.insert("error.not_found".to_string(), "Not Found".to_string());
    t.insert("error.unauthorized".to_string(), "Unauthorized".to_string());
    t.insert("error.forbidden".to_string(), "Forbidden".to_string());
    t.insert("error.server_error".to_string(), "Internal Server Error".to_string());
    t.insert("error.network_error".to_string(), "Network Error".to_string());
    t.insert("error.invalid_input".to_string(), "Invalid Input".to_string());

    t
}

fn chinese_simplified() -> HashMap<String, String> {
    let mut t = HashMap::new();

    // App
    t.insert("app.name".to_string(), "CarpAI".to_string());
    t.insert("app.tagline".to_string(), "您的AI驱动开发助手".to_string());

    // Common
    t.insert("common.ok".to_string(), "确定".to_string());
    t.insert("common.cancel".to_string(), "取消".to_string());
    t.insert("common.save".to_string(), "保存".to_string());
    t.insert("common.delete".to_string(), "删除".to_string());
    t.insert("common.edit".to_string(), "编辑".to_string());
    t.insert("common.search".to_string(), "搜索".to_string());
    t.insert("common.loading".to_string(), "加载中...".to_string());
    t.insert("common.error".to_string(), "错误".to_string());
    t.insert("common.success".to_string(), "成功".to_string());

    // Navigation
    t.insert("nav.dashboard".to_string(), "仪表盘".to_string());
    t.insert("nav.tasks".to_string(), "任务".to_string());
    t.insert("nav.plugins".to_string(), "插件".to_string());
    t.insert("nav.settings".to_string(), "设置".to_string());
    t.insert("nav.help".to_string(), "帮助".to_string());

    // Tasks
    t.insert("tasks.title".to_string(), "任务管理".to_string());
    t.insert("tasks.create".to_string(), "创建任务".to_string());
    t.insert("tasks.list".to_string(), "任务列表".to_string());
    t.insert("tasks.status.todo".to_string(), "待办".to_string());
    t.insert("tasks.status.in_progress".to_string(), "进行中".to_string());
    t.insert("tasks.status.done".to_string(), "已完成".to_string());
    t.insert("tasks.status.cancelled".to_string(), "已取消".to_string());
    t.insert("tasks.priority.low".to_string(), "低".to_string());
    t.insert("tasks.priority.medium".to_string(), "中".to_string());
    t.insert("tasks.priority.high".to_string(), "高".to_string());
    t.insert("tasks.priority.critical".to_string(), "紧急".to_string());
    t.insert("tasks.one".to_string(), "{0} 个任务".to_string());
    t.insert("tasks.other".to_string(), "{0} 个任务".to_string());

    // Plugins
    t.insert("plugins.title".to_string(), "插件管理".to_string());
    t.insert("plugins.install".to_string(), "安装插件".to_string());
    t.insert("plugins.uninstall".to_string(), "卸载插件".to_string());
    t.insert("plugins.enabled".to_string(), "已启用".to_string());
    t.insert("plugins.disabled".to_string(), "已禁用".to_string());
    t.insert("plugins.marketplace".to_string(), "插件市场".to_string());
    t.insert("plugins.search_placeholder".to_string(), "搜索插件...".to_string());

    // Dashboard
    t.insert("dashboard.title".to_string(), "仪表盘".to_string());
    t.insert("dashboard.system_status".to_string(), "系统状态".to_string());
    t.insert("dashboard.cpu_usage".to_string(), "CPU使用率".to_string());
    t.insert("dashboard.memory_usage".to_string(), "内存使用率".to_string());
    t.insert("dashboard.disk_usage".to_string(), "磁盘使用率".to_string());
    t.insert("dashboard.active_sessions".to_string(), "活跃会话".to_string());
    t.insert("dashboard.performance".to_string(), "性能指标".to_string());

    // SSH
    t.insert("ssh.title".to_string(), "SSH远程连接".to_string());
    t.insert("ssh.connect".to_string(), "连接".to_string());
    t.insert("ssh.disconnect".to_string(), "断开连接".to_string());
    t.insert("ssh.execute".to_string(), "执行命令".to_string());
    t.insert("ssh.upload".to_string(), "上传文件".to_string());
    t.insert("ssh.download".to_string(), "下载文件".to_string());
    t.insert("ssh.connected".to_string(), "已连接".to_string());
    t.insert("ssh.disconnected".to_string(), "已断开".to_string());

    // Plan Mode
    t.insert("plan_mode.title".to_string(), "计划模式".to_string());
    t.insert("plan_mode.enter".to_string(), "进入计划模式".to_string());
    t.insert("plan_mode.exit".to_string(), "退出计划模式".to_string());
    t.insert("plan_mode.step_pending".to_string(), "待处理".to_string());
    t.insert("plan_mode.step_approved".to_string(), "已批准".to_string());
    t.insert("plan_mode.step_completed".to_string(), "已完成".to_string());
    t.insert("plan_mode.step_rejected".to_string(), "已拒绝".to_string());

    // Auto Mode
    t.insert("auto_mode.title".to_string(), "自动模式".to_string());
    t.insert("auto_mode.enabled".to_string(), "自动模式已启用".to_string());
    t.insert("auto_mode.disabled".to_string(), "自动模式已禁用".to_string());
    t.insert("auto_mode.auto_approve".to_string(), "自动批准".to_string());
    t.insert("auto_mode.requires_confirmation".to_string(), "需要确认".to_string());
    t.insert("auto_mode.manual_review".to_string(), "需要人工审核".to_string());

    // Version Manager
    t.insert("version.current".to_string(), "当前版本".to_string());
    t.insert("version.install".to_string(), "安装版本".to_string());
    t.insert("version.rollback".to_string(), "回滚".to_string());
    t.insert("version.changelog".to_string(), "变更日志".to_string());
    t.insert("version.rollback_point".to_string(), "回滚点".to_string());

    // Session Export
    t.insert("export.title".to_string(), "导出会话".to_string());
    t.insert("export.format_json".to_string(), "JSON格式".to_string());
    t.insert("export.format_markdown".to_string(), "Markdown格式".to_string());
    t.insert("export.success".to_string(), "会话导出成功".to_string());

    // Errors
    t.insert("error.not_found".to_string(), "未找到".to_string());
    t.insert("error.unauthorized".to_string(), "未授权".to_string());
    t.insert("error.forbidden".to_string(), "禁止访问".to_string());
    t.insert("error.server_error".to_string(), "服务器内部错误".to_string());
    t.insert("error.network_error".to_string(), "网络错误".to_string());
    t.insert("error.invalid_input".to_string(), "无效输入".to_string());

    t
}

// 其他语言的翻译可以类似添加...
fn chinese_traditional() -> HashMap<String, String> { chinese_simplified() }
fn japanese() -> HashMap<String, String> { english() }
fn korean() -> HashMap<String, String> { english() }

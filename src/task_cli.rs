use super::task_manager::{TaskManager, TaskStatus, TaskPriority, TaskUpdates, Task};

pub struct TaskCliCommand;

impl TaskCliCommand {
    pub fn execute(args: &[String], manager: &mut TaskManager) -> String {
        if args.is_empty() {
            return Self::usage().to_string();
        }

        match args[0].as_str() {
            "create" | "add" | "new" => {
                if args.len() < 2 {
                    return "Usage: task create <title>".to_string();
                }
                let title = args[1..].join(" ");
                match manager.create(&title) {
                    Ok(task) => format!(
                        "✓ Created task {} (ID: {})\n  Title: {}\n  Status: {}\n  Priority: {}",
                        task.id[..8].to_string(), task.id, task.title,
                        task.status.display(), task.priority.display()
                    ),
                    Err(e) => format!("Error: {}", e),
                }
            }
            "list" | "ls" => Self::list_tasks(manager),
            "get" | "show" => {
                if args.len() < 2 {
                    return "Usage: task get <id>".to_string();
                }
                match manager.get(&args[1]) {
                    Some(task) => Self::format_task_detail(&task),
                    None => format!("Task '{}' not found", args[1]),
                }
            }
            "update" | "edit" => {
                if args.len() < 3 {
                    return "Usage: task update <id> --title/--status/--priority/--tags <value>".to_string();
                }
                let id = &args[1];
                let mut updates = TaskUpdates {
                    title: None,
                    description: None,
                    status: None,
                    priority: None,
                    tags: None,
                };
                let mut i = 2;
                while i < args.len() {
                    match args[i].as_str() {
                        "--title" | "-t" => {
                            if i + 1 < args.len() {
                                updates.title = Some(args[i + 1].clone());
                                i += 2;
                            } else { i += 1; }
                        }
                        "--status" | "-s" => {
                            if i + 1 < args.len() {
                                updates.status = Some(TaskStatus::from_str(&args[i + 1]));
                                i += 2;
                            } else { i += 1; }
                        }
                        "--priority" | "-p" => {
                            if i + 1 < args.len() {
                                updates.priority = Some(TaskPriority::from_str(&args[i + 1]));
                                i += 2;
                            } else { i += 1; }
                        }
                        "--tags" => {
                            if i + 1 < args.len() {
                                updates.tags = Some(args[i + 1].split(',').map(|s| s.trim().to_string()).collect());
                                i += 2;
                            } else { i += 1; }
                        }
                        _ => { i += 1; }
                    }
                }
                match manager.update(id, updates) {
                    Ok(task) => format!("✓ Updated task {} (ID: {})", task.id[..8].to_string(), task.id),
                    Err(e) => format!("Error: {}", e),
                }
            }
            "delete" | "rm" => {
                if args.len() < 2 {
                    return "Usage: task delete <id>".to_string();
                }
                match manager.delete(&args[1]) {
                    Ok(_) => format!("✓ Deleted task '{}'", args[1]),
                    Err(e) => format!("Error: {}", e),
                }
            }
            "stats" | "summary" => Self::stats(manager),
            _ => format!("Unknown subcommand: {}. {}", args[0], Self::usage()),
        }
    }

    fn list_tasks(manager: &TaskManager) -> String {
        let tasks = manager.list();
        if tasks.is_empty() {
            return "No tasks found. Create one with 'task create <title>'".to_string();
        }

        let mut output = format!("Tasks ({} total):\n\n", tasks.len());
        for task in &tasks {
            output.push_str(&format!(
                "  [{}] {} — {} ({})\n",
                task.id[..8].to_string(),
                task.status.display(),
                task.title,
                task.priority.display()
            ));
        }
        output
    }

    fn format_task_detail(task: &super::task_manager::Task) -> String {
        format!(
            r#"Task Details:
  ID: {}
  Title: {}
  Description: {}
  Status: {}
  Priority: {}
  Tags: {}
  Created: {}
  Updated: {}
"#,
            task.id,
            task.title,
            task.description.as_deref().unwrap_or("N/A"),
            task.status.display(),
            task.priority.display(),
            if task.tags.is_empty() { "None".to_string() } else { task.tags.join(", ") },
            task.created_at.format("%Y-%m-%d %H:%M:%S UTC"),
            task.updated_at.format("%Y-%m-%d %H:%M:%S UTC"),
        )
    }

    fn stats(manager: &TaskManager) -> String {
        let counts = manager.count_by_status();
        let tasks = manager.list();
        let mut output = "Task Summary:\n".to_string();

        for (status, count) in &counts {
            output.push_str(&format!("  {}: {}\n", status, count));
        }

        output.push_str(&format!("\nTotal: {} tasks\n", tasks.len()));
        output
    }

    fn usage() -> &'static str {
        r#"Task Management Commands:
  task create <title>              - Create a new task
  task list                       - List all tasks
  task get <id>                   - Show task details
  task update <id> [options]      - Update a task
  task delete <id>                - Delete a task
  task stats                      - Show task summary

Update Options:
  --title, -t <text>             Update title
  --status, -s <status>          Update status (todo/in-progress/done/cancelled)
  --priority, -p <priority>      Update priority (low/medium/high/critical)
  --tags <tag1,tag2,...>         Update tags
"#
    }
}

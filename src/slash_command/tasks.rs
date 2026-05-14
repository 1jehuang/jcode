use super::{register, SlashResult};

pub(crate) async fn register_tasks() {
    register("tasks", "List and manage tasks",
        "/tasks [create <desc>|list|status <id>]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let parts: Vec<&str> = args.trim().splitn(2, ' ').collect();
                match parts.first().copied().unwrap_or("") {
                    "create" if parts.len() >= 2 => {
                        let desc = parts[1];
                        let planner = crate::task_planner::TaskPlanner::new();
                        let plan_id = planner.create_plan("default", "Slash task", desc);
                        let task = crate::task_planner::EnhancedTask::new(desc);
                        match planner.add_task(&plan_id, task) {
                            Ok(_) => eprintln!("✅ Task created in plan: {}\n", plan_id),
                            Err(e) => eprintln!("❌ {}\n", e),
                        }
                    }
                    "list" | "ls" => {
                        let planner = crate::task_planner::TaskPlanner::new();
                        let plans = planner.list_plans();
                        eprintln!("\n📋 Tasks\n");
                        if plans.is_empty() { eprintln!("  No tasks.\n"); return; }
                        for plan_id in &plans {
                            if let Some(plan) = planner.get_plan(plan_id) {
                                eprintln!("  Plan: {} ({} tasks)", plan.name, plan.tasks.len());
                                for task_id in &plan.tasks {
                                    if let Some(t) = planner.get_task(task_id) {
                                        eprintln!("    {} — {}", t.id, t.description);
                                    }
                                }
                            }
                        }
                        eprintln!();
                    }
                    "status" | "get" if parts.len() >= 2 => {
                        let planner = crate::task_planner::TaskPlanner::new();
                        let pid = planner.find_plan_for_task(parts[1]);
                        if let Some(p) = pid {
                            if let Some(task) = planner.get_task(&parts[1]) {
                                let status = match task.status {
                                    crate::task_planner::TaskStatus::Completed => "✅ Completed",
                                    _ => "⏳ Pending",
                                };
                                eprintln!("\n📋 Task: {} — {}\n  Status:   {}\n  Priority: {}\n  Category: {}\n",
                                    task.id, task.description, status, task.priority.label(), task.category.label());
                                return;
                            }
                        }
                        eprintln!("❌ Task '{}' not found.\n", parts[1]);
                    }
                    _ => {
                        eprintln!("\n📋 Task commands:\n");
                        eprintln!("  /tasks create <desc>    Create a task\n  /tasks list              List all tasks\n  /tasks status <id>       Get task details\n  /tasks get <id>          Get task details\n");
                    }
                }
            });
            SlashResult::Ok("Task command.".into())
        }),
    ).await;
}

pub(crate) async fn register_skills() {
    register("skills", "List available skills",
        "/skills [list|search <q>|info <name>]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let parts: Vec<&str> = args.trim().splitn(2, ' ').collect();
                match parts.first().copied().unwrap_or("") {
                    "list" | "ls" | "" => {
                        eprintln!("\n📋 Available Skills\n");
                        eprintln!("  (Skill listing requires SkillRegistry.)\n");
                        eprintln!("  Use /skills info <name> for details.\n");
                    }
                    "info" if parts.len() >= 2 => {
                        eprintln!("\n📋 Skill: {}\n  (Skill info requires SkillRegistry.)\n", parts[1]);
                    }
                    _ => eprintln!("Usage: /skills [list|info <name>]\n"),
                }
            });
            SlashResult::Ok("Skills command.".into())
        }),
    ).await;
}

pub(crate) async fn register_workflows() {
    register("workflows", "Manage and run workflows",
        "/workflows [list|run <name>]",
        std::sync::Arc::new(|args: &str| {
            spawn_async(move || async move {
                let parts: Vec<&str> = args.trim().splitn(2, ' ').collect();
                match parts.first().copied().unwrap_or("") {
                    "list" | "ls" | "" => {
                        eprintln!("\n📋 Workflows\n  (Use /workflows run <name> to execute.)\n");
                    }
                    "run" if parts.len() >= 2 => {
                        eprintln!("\n⚡ Running workflow: {}\n  (Workflow execution requires workflow engine.)\n", parts[1]);
                    }
                    _ => eprintln!("Usage: /workflows [list|run <name>]\n"),
                }
            });
            SlashResult::Ok("Workflows command.".into())
        }),
    ).await;
}

fn spawn_async<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}

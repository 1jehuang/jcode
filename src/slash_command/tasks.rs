use super::{register, SlashResult};

pub(crate) async fn register_tasks() {
    register("tasks", "List and manage tasks", "/tasks [create <desc>|list|status <id>]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            spawn(move || async move {
                let parts: Vec<&str> = a.trim().splitn(2, ' ').collect();
                match parts.first().copied().unwrap_or("") {
                    "create" if parts.len() >= 2 => {
                        let desc = parts[1];
                        let planner = crate::task_planner::TaskPlanner::new();
                        let plan_id = planner.create_plan("default", "Slash task", desc);
                        let task = crate::task_planner::EnhancedTask::new(desc);
                        match planner.add_task(&plan_id, task) {
                            Ok(_) => eprintln!("✅ Task created: {}\n", plan_id),
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
                        if let Some(pid) = planner.find_plan_for_task(parts[1]) {
                            if let Some(task) = planner.get_task(&parts[1]) {
                                let status = match task.status { crate::task_planner::TaskStatus::Completed => "✅", _ => "⏳" };
                                eprintln!("\n📋 {} — {}\n  Status: {}\n  Priority: {}\n", task.id, task.description, status, task.priority.label());
                                return;
                            }
                        }
                        eprintln!("❌ Task '{}' not found.\n", parts[1]);
                    }
                    _ => eprintln!("Usage: /tasks [create|list|status]\n"),
                }
            });
            SlashResult::Ok("Task.".into())
        }),
    ).await;
}

pub(crate) async fn register_skills() {
    register("skills", "List available skills", "/skills [list|info <name>]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            spawn(move || async move {
                let parts: Vec<&str> = a.trim().splitn(2, ' ').collect();
                let reg = crate::skill::SkillRegistry::shared_registry().read().await;
                match parts.first().copied().unwrap_or("") {
                    "list"|"ls"|"" => {
                        let skills = reg.list();
                        eprintln!("\n📋 Skills ({})\n", skills.len());
                        for s in skills { eprintln!("  • {} — {}", s.name, s.description); }
                        eprintln!();
                    }
                    "info" if parts.len()>=2 => {
                        match reg.get(parts[1]) {
                            Some(s) => eprintln!("\n📋 {}\n  {}\n  Path: {}\n", s.name, s.description, s.path.display()),
                            None => eprintln!("❌ Not found.\n"),
                        }
                    }
                    _ => eprintln!("Usage: /skills [list|info <name>]\n"),
                }
            });
            SlashResult::Ok("Skills.".into())
        }),
    ).await;
}

pub(crate) async fn register_workflows() {
    register("workflows", "Manage and run workflows", "/workflows [list|run <name>]",
        std::sync::Arc::new(|args: &str| {
            let a = args.to_string();
            spawn(move || async move {
                let parts: Vec<&str> = a.trim().splitn(2, ' ').collect();
                match parts.first().copied().unwrap_or("") {
                    "list"|"ls"|"" => {
                        let t = crate::workflow::WorkflowTemplate::all();
                        eprintln!("\n📋 Workflows ({})\n", t.len());
                        for w in &t { eprintln!("  • {} — {}", w.name, w.description); }
                        eprintln!("\n  /workflows run <name>\n");
                    }
                    "run" if parts.len()>=2 => {
                        match crate::workflow::WorkflowTemplate::to_config(parts[1]) {
                            Some(cfg) => {
                                let r = crate::workflow::runner::WorkflowRunner::new();
                                let id = r.register(cfg).await;
                                eprintln!("\n⚡ Running '{}' (id: {:?})\n", parts[1], id);
                            }
                            None => eprintln!("❌ Workflow '{}' not found.\n", parts[1]),
                        }
                    }
                    _ => eprintln!("Usage: /workflows [list|run <name>]\n"),
                }
            });
            SlashResult::Ok("Workflows.".into())
        }),
    ).await;
}

fn spawn<F, Fut>(f: F) where F: FnOnce() -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send + 'static {
    if let Ok(h) = tokio::runtime::Handle::try_current() { h.spawn(f()); }
}

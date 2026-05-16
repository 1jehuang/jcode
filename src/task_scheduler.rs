use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use cron::Schedule;
use std::str::FromStr;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::task::{JoinHandle, AbortHandle};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    pub cron_expression: String,
    pub task_type: TaskType,
    pub payload: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: DateTime<Utc>,
    pub run_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskType {
    Command,
    Script,
    Webhook,
    Workflow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecution {
    pub task_id: String,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub result: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskScheduler {
    tasks: Arc<RwLock<HashMap<String, ScheduledTask>>>,
    runners: Arc<RwLock<HashMap<String, (JoinHandle<()>, AbortHandle)>>>,
    tx: mpsc::Sender<TaskExecution>,
    rx: mpsc::Receiver<TaskExecution>,
    executor: Option<JoinHandle<()>>,
    is_running: Arc<RwLock<bool>>,
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskScheduler {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            runners: Arc::new(RwLock::new(HashMap::new())),
            tx,
            rx,
            executor: None,
            is_running: Arc::new(RwLock::new(false)),
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        let mut is_running = self.is_running.write().await;
        if *is_running {
            return Err(anyhow!("Scheduler is already running"));
        }
        *is_running = true;

        let tx_clone = self.tx.clone();
        let tasks_clone = self.tasks.clone();
        let runners_clone = self.runners.clone();
        let is_running_clone = self.is_running.clone();

        self.executor = Some(tokio::spawn(async move {
            Self::executor_loop(tx_clone, tasks_clone, runners_clone, is_running_clone).await;
        }));

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        let mut is_running = self.is_running.write().await;
        if !*is_running {
            return Err(anyhow!("Scheduler is not running"));
        }
        *is_running = false;

        if let Some(executor) = self.executor.take() {
            executor.abort();
        }

        let mut runners = self.runners.write().await;
        for (_, (_, abort_handle)) in runners.drain() {
            abort_handle.abort();
        }

        Ok(())
    }

    pub async fn add_task(&self, task: ScheduledTask) -> Result<()> {
        let schedule = <Schedule as FromStr>::from_str(&task.cron_expression).map_err(|e| anyhow!(e))?;
        let next_run = schedule.upcoming(Utc).next().ok_or_else(|| anyhow!("Invalid cron expression"))?;

        let mut tasks = self.tasks.write().await;
        let mut task = task;
        task.next_run = next_run;
        
        if tasks.contains_key(&task.id) {
            return Err(anyhow!("Task with id {} already exists", task.id));
        }

        tasks.insert(task.id.clone(), task);
        Ok(())
    }

    pub async fn remove_task(&self, task_id: &str) -> Result<ScheduledTask> {
        let mut tasks = self.tasks.write().await;
        tasks.remove(task_id).ok_or_else(|| anyhow!("Task not found"))
    }

    pub async fn update_task(&self, task_id: &str, updated: ScheduledTask) -> Result<()> {
        let schedule = <Schedule as FromStr>::from_str(&updated.cron_expression).map_err(|e| anyhow!(e))?;
        let next_run = schedule.upcoming(Utc).next().ok_or_else(|| anyhow!("Invalid cron expression"))?;

        let mut tasks = self.tasks.write().await;
        let task = tasks.get_mut(task_id).ok_or_else(|| anyhow!("Task not found"))?;

        task.name = updated.name;
        task.cron_expression = updated.cron_expression;
        task.task_type = updated.task_type;
        task.payload = updated.payload;
        task.enabled = updated.enabled;
        task.next_run = next_run;

        Ok(())
    }

    pub async fn get_task(&self, task_id: &str) -> Option<ScheduledTask> {
        self.tasks.read().await.get(task_id).cloned()
    }

    pub async fn get_all_tasks(&self) -> Vec<ScheduledTask> {
        self.tasks.read().await.values().cloned().collect()
    }

    pub async fn enable_task(&self, task_id: &str) -> Result<()> {
        let mut tasks = self.tasks.write().await;
        let task = tasks.get_mut(task_id).ok_or_else(|| anyhow!("Task not found"))?;
        task.enabled = true;
        Ok(())
    }

    pub async fn disable_task(&self, task_id: &str) -> Result<()> {
        let mut tasks = self.tasks.write().await;
        let task = tasks.get_mut(task_id).ok_or_else(|| anyhow!("Task not found"))?;
        task.enabled = false;
        Ok(())
    }

    async fn executor_loop(
        tx: mpsc::Sender<TaskExecution>,
        tasks: Arc<RwLock<HashMap<String, ScheduledTask>>>,
        runners: Arc<RwLock<HashMap<String, (JoinHandle<()>, AbortHandle)>>>,
        is_running: Arc<RwLock<bool>>,
    ) {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        while *is_running.read().await {
            interval.tick().await;

            let now = Utc::now();
            let tasks_snapshot = tasks.read().await.clone();

            for (task_id, task) in tasks_snapshot.iter() {
                if !task.enabled {
                    continue;
                }

                if now >= task.next_run {
                    let runners_lock = runners.read().await;
                    if runners_lock.contains_key(task_id) {
                        continue;
                    }
                    drop(runners_lock);

                    let tx_clone = tx.clone();
                    let tasks_clone = tasks.clone();
                    let task_clone = task.clone();

                    let handle = tokio::spawn(async move {
                        let result = Self::execute_task(&task_clone).await;
                        let _ = Self::update_task_after_execution(&tasks_clone, &task_clone.id, result.timestamp).await;
                        let _ = tx_clone.send(result).await;
                    });

                    let abort_handle = handle.abort_handle();
                    let mut runners_write = runners.write().await;
                    runners_write.insert(task_id.clone(), (handle, abort_handle));
                }
            }

            let mut runners_write = runners.write().await;
            let completed: HashSet<String> = runners_write
                .iter()
                .filter(|(_, (handle, _))| handle.is_finished())
                .map(|(id, _)| id.clone())
                .collect();

            for id in completed {
                runners_write.remove(&id);
            }
        }
    }

    async fn execute_task(task: &ScheduledTask) -> TaskExecution {
        let timestamp = Utc::now();

        let result = match task.task_type {
            TaskType::Command => Self::execute_command(task).await,
            TaskType::Script => Self::execute_script(task).await,
            TaskType::Webhook => Self::execute_webhook(task).await,
            TaskType::Workflow => Self::execute_workflow(task).await,
        };

        TaskExecution {
            task_id: task.id.clone(),
            timestamp,
            success: result.is_ok(),
            result: result.as_ref().ok().cloned(),
            error: result.err().map(|e| e.to_string()),
        }
    }

    async fn execute_command(task: &ScheduledTask) -> Result<String> {
        let command = task.payload.get("command").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Command not specified"))?;

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(anyhow!(String::from_utf8_lossy(&output.stderr).to_string()))
        }
    }

    async fn execute_script(_task: &ScheduledTask) -> Result<String> {
        Err(anyhow!("Script execution not implemented"))
    }

    async fn execute_webhook(task: &ScheduledTask) -> Result<String> {
        let url = task.payload.get("url").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("URL not specified"))?;

        let client = reqwest::Client::new();
        let response = client.get(url).send().await.map_err(|e| anyhow!(e))?;
        
        if response.status().is_success() {
            response.text().await.map_err(|e| anyhow!(e))
        } else {
            Err(anyhow!("HTTP request failed: {}", response.status()))
        }
    }

    async fn execute_workflow(_task: &ScheduledTask) -> Result<String> {
        Err(anyhow!("Workflow execution not implemented"))
    }

    async fn update_task_after_execution(tasks: &Arc<RwLock<HashMap<String, ScheduledTask>>>, task_id: &str, timestamp: DateTime<Utc>) -> Result<()> {
        let mut tasks_lock = tasks.write().await;
        
        if let Some(task) = tasks_lock.get_mut(task_id) {
            let schedule = <Schedule as FromStr>::from_str(&task.cron_expression).map_err(|e| anyhow!(e))?;
            task.last_run = Some(timestamp);
            task.next_run = schedule.upcoming(Utc).next().ok_or_else(|| anyhow!("Invalid cron"))?;
            task.run_count += 1;
        }
        
        Ok(())
    }

    pub fn parse_cron(expression: &str) -> Result<Schedule> {
        <Schedule as FromStr>::from_str(expression).map_err(|e: cron::error::Error| anyhow!(e))
    }

    pub fn get_next_run(expression: &str) -> Result<DateTime<Utc>> {
        let schedule = Self::parse_cron(expression)?;
        schedule.upcoming(Utc).next().ok_or_else(|| anyhow!("No upcoming run"))
    }
}
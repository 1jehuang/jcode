use parking_lot::RwLock;
use std::collections::{HashMap, HashSet, BinaryHeap};
use std::sync::Arc;
use tokio::sync::Notify;
use uuid::Uuid;

pub type TaskId = Uuid;
pub type ResourceId = Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub description: String,
    pub priority: TaskPriority,
    pub dependencies: Vec<TaskId>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    Low = 1,
    Medium = 2,
    High = 3,
    Urgent = 4,
}

#[derive(Debug, Clone)]
pub enum AgentRole {
    Coordinator,
    Worker,
    Specialist(String),
}

#[derive(Debug, Clone)]
pub struct TaskRequirements {
    pub task_id: TaskId,
    pub role: AgentRole,
    pub model: String,
    pub priority: TaskPriority,
    pub dependencies: Vec<TaskId>,
    pub resources: ResourceRequirements,
}

#[derive(Debug, Clone)]
pub struct ResourceRequirements {
    pub cpu: usize,
    pub gpu: usize,
    pub memory: usize,
    pub network: usize,
}

#[derive(Debug, Clone)]
pub struct Resource {
    pub id: ResourceId,
    pub name: String,
    pub cpu_cores: usize,
    pub gpu_count: usize,
    pub memory_gb: usize,
    pub network_mbps: usize,
    pub status: ResourceStatus,
    pub current_load: ResourceLoad,
}

#[derive(Debug, Clone)]
pub struct ResourceLoad {
    pub cpu_usage: f32,
    pub memory_usage: f32,
    pub gpu_usage: f32,
    pub network_usage: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceStatus {
    Available,
    Busy,
    Offline,
    Degraded,
}

#[derive(Debug, Clone)]
pub struct ResourceAllocation {
    pub task_id: TaskId,
    pub resource_id: ResourceId,
    pub allocation: ResourceAllocationDetails,
}

#[derive(Debug, Clone)]
pub struct ResourceAllocationDetails {
    pub cpu_cores: usize,
    pub gpu_count: usize,
    pub memory_gb: usize,
    pub network_mbps: usize,
}

#[derive(Debug, Clone)]
pub struct TaskExecutionResult {
    pub task_id: TaskId,
    pub resource_id: ResourceId,
    pub status: TaskStatus,
    pub result: String,
    pub execution_time_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub enum SchedulerError {
    NoAvailableResources,
    TaskAnalysisFailed(String),
    ResourceAllocationFailed,
    ExecutionFailed(String),
    TaskNotFound,
    DependencyFailed(TaskId),
    TaskCancelled,
}

impl std::fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SchedulerError::NoAvailableResources => write!(f, "No available resources"),
            SchedulerError::TaskAnalysisFailed(msg) => write!(f, "Task analysis failed: {}", msg),
            SchedulerError::ResourceAllocationFailed => write!(f, "Resource allocation failed"),
            SchedulerError::ExecutionFailed(msg) => write!(f, "Execution failed: {}", msg),
            SchedulerError::TaskNotFound => write!(f, "Task not found"),
            SchedulerError::DependencyFailed(id) => write!(f, "Dependency task {} failed", id),
            SchedulerError::TaskCancelled => write!(f, "Task was cancelled"),
        }
    }
}

impl std::error::Error for SchedulerError {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueuedTask {
    task: Task,
    priority: TaskPriority,
    dependencies_resolved: bool,
}

impl PartialOrd for QueuedTask {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedTask {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.priority.cmp(&other.priority) {
            std::cmp::Ordering::Equal => {
                self.task.id.as_u128().cmp(&other.task.id.as_u128()).reverse()
            }
            ord => ord,
        }
    }
}

pub struct UnifiedScheduler {
    task_analyzer: TaskAnalyzer,
    resource_manager: ResourceManager,
    optimizer: Optimizer,
    executor: Executor,
    task_queue: RwLock<BinaryHeap<QueuedTask>>,
    task_status: RwLock<HashMap<TaskId, TaskStatus>>,
    task_dependencies: RwLock<HashMap<TaskId, HashSet<TaskId>>>,
    completed_tasks: RwLock<HashSet<TaskId>>,
    running_tasks: RwLock<HashSet<TaskId>>,
    notify: Arc<Notify>,
    shutdown: RwLock<bool>,
}

impl Default for UnifiedScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl UnifiedScheduler {
    pub fn new() -> Self {
        Self {
            task_analyzer: TaskAnalyzer::new(),
            resource_manager: ResourceManager::new(),
            optimizer: Optimizer::new(),
            executor: Executor::new(),
            task_queue: RwLock::new(BinaryHeap::new()),
            task_status: RwLock::new(HashMap::new()),
            task_dependencies: RwLock::new(HashMap::new()),
            completed_tasks: RwLock::new(HashSet::new()),
            running_tasks: RwLock::new(HashSet::new()),
            notify: Arc::new(Notify::new()),
            shutdown: RwLock::new(false),
        }
    }

    pub async fn schedule(&self, task: Task) -> Result<TaskId, SchedulerError> {
        let task_id = task.id;
        
        self.task_status.write().insert(task_id, TaskStatus::Pending);
        
        if !task.dependencies.is_empty() {
            let mut deps = HashSet::new();
            for dep_id in &task.dependencies {
                deps.insert(*dep_id);
            }
            self.task_dependencies.write().insert(task_id, deps);
        }

        let requirements = self.task_analyzer.analyze(&task).await?;
        
        let deps_resolved = self.check_dependencies(&task).await;
        
        let queued_task = QueuedTask {
            task,
            priority: requirements.priority,
            dependencies_resolved: deps_resolved,
        };
        
        self.task_queue.write().push(queued_task);
        self.task_status.write().insert(task_id, TaskStatus::Queued);
        
        self.notify.notify_one();
        
        Ok(task_id)
    }

    async fn check_dependencies(&self, task: &Task) -> bool {
        if task.dependencies.is_empty() {
            return true;
        }
        
        let completed = self.completed_tasks.read();
        task.dependencies.iter().all(|dep_id| completed.contains(dep_id))
    }

    pub async fn run(&self) {
        loop {
            if *self.shutdown.read() {
                break;
            }

            self.notify.notified().await;

            let mut queue = self.task_queue.write();
            let mut ready_tasks = Vec::new();

            let mut remaining = Vec::new();
            while let Some(queued_task) = queue.pop() {
                let deps_resolved = if queued_task.dependencies_resolved {
                    true
                } else {
                    self.check_dependencies(&queued_task.task).await
                };

                if deps_resolved {
                    ready_tasks.push(queued_task);
                } else {
                    remaining.push(QueuedTask {
                        task: queued_task.task,
                        priority: queued_task.priority,
                        dependencies_resolved: false,
                    });
                }
            }

            for task in remaining {
                queue.push(task);
            }

            drop(queue);

            for queued_task in ready_tasks {
                let task = queued_task.task;
                if *self.shutdown.read() {
                    break;
                }

                let result = self.execute_task(&task).await;
                
                self.running_tasks.write().remove(&task.id);
                self.completed_tasks.write().insert(task.id);
                
                match result {
                    Ok(_) => {
                        self.task_status.write().insert(task.id, TaskStatus::Completed);
                    }
                    Err(_) => {
                        self.task_status.write().insert(task.id, TaskStatus::Failed);
                        self.mark_dependents_failed(task.id);
                    }
                }
                
                self.notify.notify_one();
            }
        }
    }

    async fn execute_task(&self, task: &Task) -> Result<TaskExecutionResult, SchedulerError> {
        let task_requirements = self.task_analyzer.analyze(task).await?;
        let resources = self.resource_manager.get_resources().await?;
        
        let allocation = self.optimizer.optimize(&task_requirements, &resources).await?;
        
        self.resource_manager.update_resource_status(allocation.resource_id, ResourceStatus::Busy).await;
        self.running_tasks.write().insert(task.id);
        
        let result = self.executor.execute(task, &allocation).await;
        
        self.resource_manager.update_resource_status(allocation.resource_id, ResourceStatus::Available).await;
        
        result
    }

    fn mark_dependents_failed(&self, failed_task_id: TaskId) {
        let mut to_process = vec![failed_task_id];
        
        while let Some(task_id) = to_process.pop() {
            let dependencies = self.task_dependencies.read().clone();
            
            for (dep_task_id, deps) in dependencies.iter() {
                if deps.contains(&task_id) {
                    self.task_status.write().insert(*dep_task_id, TaskStatus::Failed);
                    to_process.push(*dep_task_id);
                }
            }
        }
    }

    pub fn get_task_status(&self, task_id: TaskId) -> Option<TaskStatus> {
        self.task_status.read().get(&task_id).copied()
    }

    pub fn cancel_task(&self, task_id: TaskId) -> bool {
        let mut status = self.task_status.write();
        if let Some(s) = status.get_mut(&task_id)
            && (*s == TaskStatus::Pending || *s == TaskStatus::Queued) {
                *s = TaskStatus::Cancelled;
                self.mark_dependents_failed(task_id);
                return true;
            }
        false
    }

    pub fn shutdown(&self) {
        *self.shutdown.write() = true;
        self.notify.notify_one();
    }

    pub fn get_stats(&self) -> SchedulerStats {
        let queue = self.task_queue.read();
        let status = self.task_status.read();
        let running = self.running_tasks.read();
        let completed = self.completed_tasks.read();
        
        let pending = status.values().filter(|&&s| s == TaskStatus::Pending).count();
        let queued = status.values().filter(|&&s| s == TaskStatus::Queued).count();
        let running_count = running.len();
        let completed_count = completed.len();
        let failed = status.values().filter(|&&s| s == TaskStatus::Failed).count();
        
        SchedulerStats {
            pending_tasks: pending,
            queued_tasks: queued,
            running_tasks: running_count,
            completed_tasks: completed_count,
            failed_tasks: failed,
            total_queued: queue.len(),
        }
    }
}

pub struct SchedulerStats {
    pub pending_tasks: usize,
    pub queued_tasks: usize,
    pub running_tasks: usize,
    pub completed_tasks: usize,
    pub failed_tasks: usize,
    pub total_queued: usize,
}

pub struct TaskAnalyzer;

impl Default for TaskAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskAnalyzer {
    pub fn new() -> Self {
        Self
    }

    pub async fn analyze(&self, task: &Task) -> Result<TaskRequirements, SchedulerError> {
        let role = self.detect_role(task);
        let model = self.analyze_model_requirement(task);
        let priority = self.evaluate_priority(task);
        let dependencies = task.dependencies.clone();
        let resources = self.estimate_resource_requirements(task);

        Ok(TaskRequirements {
            task_id: task.id,
            role,
            model,
            priority,
            dependencies,
            resources,
        })
    }

    fn detect_role(&self, task: &Task) -> AgentRole {
        if task.description.contains("分析") || task.description.contains("设计") || task.description.contains("协调") {
            AgentRole::Coordinator
        } else if task.description.contains("代码") || task.description.contains("实现") || task.description.contains("编写") {
            AgentRole::Worker
        } else if task.description.contains("搜索") || task.description.contains("查询") {
            AgentRole::Specialist("search".to_string())
        } else if task.description.contains("测试") || task.description.contains("验证") {
            AgentRole::Specialist("tester".to_string())
        } else {
            AgentRole::Specialist("general".to_string())
        }
    }

    fn analyze_model_requirement(&self, task: &Task) -> String {
        let desc = &task.description;
        if desc.contains("复杂") || desc.contains("架构") || desc.contains("大规模") || desc.contains("推理") {
            "qwen-3.6-max".to_string()
        } else if desc.contains("代码") || desc.contains("编程") {
            "qwen-3.6-14b".to_string()
        } else {
            "qwen-3.6-7b".to_string()
        }
    }

    fn evaluate_priority(&self, task: &Task) -> TaskPriority {
        task.priority
    }

    fn estimate_resource_requirements(&self, task: &Task) -> ResourceRequirements {
        let (cpu, gpu, memory, network) = match task.priority {
            TaskPriority::Urgent => (16, 2, 64, 200),
            TaskPriority::High => (8, 1, 32, 100),
            TaskPriority::Medium => (4, 1, 16, 50),
            TaskPriority::Low => (2, 0, 8, 20),
        };

        ResourceRequirements { cpu, gpu, memory, network }
    }
}

pub struct ResourceManager {
    resources: RwLock<Vec<Resource>>,
    monitors: RwLock<HashMap<ResourceId, ResourceMonitor>>,
}

impl Default for ResourceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceManager {
    pub fn new() -> Self {
        Self {
            resources: RwLock::new(Vec::new()),
            monitors: RwLock::new(HashMap::new()),
        }
    }

    pub async fn get_resources(&self) -> Result<Vec<Resource>, SchedulerError> {
        self.update_resource_loads().await;
        Ok(self.resources.read().clone())
    }

    pub fn register_resource(&self, resource: Resource) {
        let mut resources = self.resources.write();
        if !resources.iter().any(|r| r.id == resource.id) {
            resources.push(resource.clone());
            self.monitors.write().insert(resource.id, ResourceMonitor::new(resource.id));
        }
    }

    pub async fn update_resource_status(&self, resource_id: ResourceId, status: ResourceStatus) {
        let mut resources = self.resources.write();
        if let Some(r) = resources.iter_mut().find(|r| r.id == resource_id) {
            r.status = status;
        }
    }

    pub async fn update_resource_loads(&self) {
        let mut resources = self.resources.write();
        for resource in resources.iter_mut() {
            if let Some(monitor) = self.monitors.read().get(&resource.id) {
                resource.current_load = monitor.get_load().await;
            }
        }
    }

    pub fn get_resource_stats(&self) -> Vec<ResourceStats> {
        let resources = self.resources.read();
        resources.iter().map(|r| ResourceStats {
            id: r.id,
            name: r.name.clone(),
            status: r.status,
            load: r.current_load.clone(),
            cpu_cores: r.cpu_cores,
            memory_gb: r.memory_gb,
        }).collect()
    }
}

pub struct ResourceMonitor {
    resource_id: ResourceId,
}

impl ResourceMonitor {
    pub fn new(resource_id: ResourceId) -> Self {
        Self { resource_id }
    }

    pub async fn get_load(&self) -> ResourceLoad {
        ResourceLoad {
            cpu_usage: self.simulate_cpu_usage().await,
            memory_usage: self.simulate_memory_usage().await,
            gpu_usage: self.simulate_gpu_usage().await,
            network_usage: self.simulate_network_usage().await,
        }
    }

    async fn simulate_cpu_usage(&self) -> f32 {
        25.0 + rand::random::<f32>() * 30.0
    }

    async fn simulate_memory_usage(&self) -> f32 {
        30.0 + rand::random::<f32>() * 40.0
    }

    async fn simulate_gpu_usage(&self) -> f32 {
        10.0 + rand::random::<f32>() * 50.0
    }

    async fn simulate_network_usage(&self) -> f32 {
        15.0 + rand::random::<f32>() * 25.0
    }
}

pub struct ResourceStats {
    pub id: ResourceId,
    pub name: String,
    pub status: ResourceStatus,
    pub load: ResourceLoad,
    pub cpu_cores: usize,
    pub memory_gb: usize,
}

pub struct Optimizer {
    scheduling_strategy: SchedulingStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingStrategy {
    FirstFit,
    BestFit,
    WorstFit,
    LoadBalanced,
}

impl Default for Optimizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Optimizer {
    pub fn new() -> Self {
        Self {
            scheduling_strategy: SchedulingStrategy::LoadBalanced,
        }
    }

    pub fn set_strategy(&mut self, strategy: SchedulingStrategy) {
        self.scheduling_strategy = strategy;
    }

    pub async fn optimize(
        &self,
        requirements: &TaskRequirements,
        resources: &[Resource],
    ) -> Result<ResourceAllocation, SchedulerError> {
        let available_resources: Vec<&Resource> = resources
            .iter()
            .filter(|r| r.status == ResourceStatus::Available)
            .filter(|r| self.meets_requirements(r, requirements))
            .filter(|r| self.is_resource_healthy(r))
            .collect();

        if available_resources.is_empty() {
            return Err(SchedulerError::NoAvailableResources);
        }

        let best_resource = match self.scheduling_strategy {
            SchedulingStrategy::FirstFit => self.select_first_fit(&available_resources),
            SchedulingStrategy::BestFit => self.select_best_fit(&available_resources, requirements),
            SchedulingStrategy::WorstFit => self.select_worst_fit(&available_resources, requirements),
            SchedulingStrategy::LoadBalanced => self.select_load_balanced(&available_resources),
        };

        Ok(ResourceAllocation {
            task_id: requirements.task_id,
            resource_id: best_resource.id,
            allocation: ResourceAllocationDetails {
                cpu_cores: requirements.resources.cpu,
                gpu_count: requirements.resources.gpu,
                memory_gb: requirements.resources.memory,
                network_mbps: requirements.resources.network,
            },
        })
    }

    fn meets_requirements(&self, resource: &Resource, requirements: &TaskRequirements) -> bool {
        resource.cpu_cores >= requirements.resources.cpu
            && resource.gpu_count >= requirements.resources.gpu
            && resource.memory_gb >= requirements.resources.memory
            && resource.network_mbps >= requirements.resources.network
    }

    fn is_resource_healthy(&self, resource: &Resource) -> bool {
        resource.current_load.cpu_usage < 80.0
            && resource.current_load.memory_usage < 85.0
            && resource.current_load.gpu_usage < 90.0
    }

    fn select_first_fit<'a>(&self, resources: &[&'a Resource]) -> &'a Resource {
        resources[0]
    }

    fn select_best_fit<'a>(
        &self,
        resources: &[&'a Resource],
        requirements: &TaskRequirements,
    ) -> &'a Resource {
        resources.iter()
            .min_by_key(|r| {
                let cpu_score = (r.cpu_cores as i64 - requirements.resources.cpu as i64).abs();
                let mem_score = (r.memory_gb as i64 - requirements.resources.memory as i64).abs();
                let gpu_score = (r.gpu_count as i64 - requirements.resources.gpu as i64).abs();
                cpu_score + mem_score + gpu_score
            })
            .unwrap_or_else(|| {
                panic!("select_best_fit called with empty resource list")
            })
    }

    fn select_worst_fit<'a>(
        &self,
        resources: &[&'a Resource],
        requirements: &TaskRequirements,
    ) -> &'a Resource {
        resources.iter()
            .max_by_key(|r| {
                let cpu_score = (r.cpu_cores as i64 - requirements.resources.cpu as i64).abs();
                let mem_score = (r.memory_gb as i64 - requirements.resources.memory as i64).abs();
                cpu_score + mem_score
            })
            .unwrap_or_else(|| {
                panic!("select_worst_fit called with empty resource list")
            })
    }

    fn select_load_balanced<'a>(&self, resources: &[&'a Resource]) -> &'a Resource {
        resources.iter()
            .min_by(|a, b| {
                let a_load = (a.current_load.cpu_usage + a.current_load.memory_usage + a.current_load.gpu_usage) / 3.0;
                let b_load = (b.current_load.cpu_usage + b.current_load.memory_usage + b.current_load.gpu_usage) / 3.0;
                a_load.partial_cmp(&b_load).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or_else(|| {
                panic!("select_load_balanced called with empty resource list")
            })
    }
}

pub struct Executor;

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(
        &self,
        task: &Task,
        allocation: &ResourceAllocation,
    ) -> Result<TaskExecutionResult, SchedulerError> {
        let execution_time = 1000 + rand::random::<u64>() % 5000;
        tokio::time::sleep(tokio::time::Duration::from_millis(execution_time)).await;

        if rand::random::<f32>() < 0.05 {
            return Err(SchedulerError::ExecutionFailed("Random failure for testing".to_string()));
        }

        Ok(TaskExecutionResult {
            task_id: task.id,
            resource_id: allocation.resource_id,
            status: TaskStatus::Completed,
            result: format!("Task {} executed successfully on resource {}", task.id, allocation.resource_id),
            execution_time_ms: execution_time,
            error: None,
        })
    }
}

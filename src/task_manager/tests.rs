#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_creation() {
        let manager = TaskManager::new();
        let task = manager.create("Implement user authentication").unwrap();

        assert!(!task.id.is_empty());
        assert_eq!(task.title, "Implement user authentication");
        assert_eq!(task.status, TaskStatus::Todo);
        assert_eq!(task.priority, TaskPriority::Medium);
    }

    #[test]
    fn test_task_list_and_get() {
        let manager = TaskManager::new();
        let task1 = manager.create("Task 1").unwrap();
        let task2 = manager.create("Task 2").unwrap();

        let tasks = manager.list();
        assert_eq!(tasks.len(), 2);

        let retrieved = manager.get(&task1.id).unwrap();
        assert_eq!(retrieved.title, "Task 1");
    }

    #[test]
    fn test_task_update() {
        let manager = TaskManager::new();
        let task = manager.create("Original title").unwrap();

        let updated = manager.update(&task.id, TaskUpdates {
            title: Some("Updated title".to_string()),
            status: Some(TaskStatus::InProgress),
            priority: Some(TaskPriority::High),
            tags: Some(vec!["backend".to_string(), "urgent".to_string()]),
        }).unwrap();

        assert_eq!(updated.title, "Updated title");
        assert_eq!(updated.status, TaskStatus::InProgress);
        assert_eq!(updated.priority, TaskPriority::High);
        assert_eq!(updated.tags.len(), 2);
    }

    #[test]
    fn test_task_delete() {
        let manager = TaskManager::new();
        let task = manager.create("To be deleted").unwrap();

        let result = manager.delete(&task.id);
        assert!(result.is_ok());

        let tasks = manager.list();
        assert!(tasks.is_empty());
    }

    #[test]
    fn test_task_delete_nonexistent() {
        let manager = TaskManager::new();
        let result = manager.delete("nonexistent-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_task_status_transitions() {
        let manager = TaskManager::new();
        let task = manager.create("Test status").unwrap();

        manager.update(&task.id, TaskUpdates {
            status: Some(TaskStatus::InProgress),
            ..Default::default()
        }).ok();

        let updated = manager.get(&task.id).unwrap();
        assert_eq!(updated.status, TaskStatus::InProgress);

        manager.update(&task.id, TaskUpdates {
            status: Some(TaskStatus::Done),
            ..Default::default()
        }).ok();

        let completed = manager.get(&task.id).unwrap();
        assert_eq!(completed.status, TaskStatus::Done);
    }

    #[test]
    fn test_task_priority_ordering() {
        let manager = TaskManager::new();
        manager.create("Low priority").ok();
        manager.update(&manager.list()[0].id, TaskUpdates {
            priority: Some(TaskPriority::Low),
            ..Default::default()
        }).ok();

        manager.create("Critical priority").ok();
        manager.update(&manager.list()[1].id, TaskUpdates {
            priority: Some(TaskPriority::Critical),
            ..Default::default()
        }).ok();

        let tasks = manager.list();
        assert_eq!(tasks[0].priority, TaskPriority::Critical);
        assert_eq!(tasks[1].priority, TaskPriority::Low);
    }

    #[test]
    fn test_task_stats() {
        let manager = TaskManager::new();
        manager.create("Todo task").ok();
        manager.create("In-progress task").ok();
        manager.update(&manager.list()[1].id, TaskUpdates {
            status: Some(TaskStatus::InProgress),
            ..Default::default()
        }).ok();

        let stats = manager.count_by_status();
        assert!(stats.contains_key(&"⬜ Todo".to_string()));
        assert!(stats.contains_key(&"🔄 In Progress".to_string()));
    }
}

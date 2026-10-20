//! Stage and lane layout for a todo list, shared by the swarm plan builder and
//! every renderer so execution order and display can never disagree.
//!
//! * Stages run down, in order of first appearance in the list. Every todo
//!   with the same `stage` number joins one stage, wherever it appears.
//! * A todo without a `stage` is a stage of its own, so an unstaged list stays
//!   a plain sequential checklist.
//! * Lanes run across. Inside a stage, todos that share a `group` form one
//!   lane that runs in list order. Different lanes run in parallel. An
//!   ungrouped todo is a lane of its own.

use crate::TodoItem;

/// One parallel lane inside a stage: indices into the original todo slice,
/// in list order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoLane {
    pub group: Option<String>,
    pub items: Vec<usize>,
}

/// One stage: lanes that run in parallel, in first-appearance order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoStage {
    /// The model-written stage number, `None` for an unstaged todo.
    pub number: Option<u32>,
    pub lanes: Vec<TodoLane>,
}

impl TodoStage {
    pub fn item_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.lanes
            .iter()
            .flat_map(|lane| lane.items.iter().copied())
    }

    pub fn len(&self) -> usize {
        self.lanes.iter().map(|lane| lane.items.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Trimmed, non-empty group label.
pub fn lane_group(todo: &TodoItem) -> Option<String> {
    todo.group
        .as_deref()
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .map(str::to_string)
}

/// True when any todo declares a stage, which switches renderers to the
/// stage grid.
pub fn has_stages<'a>(todos: impl IntoIterator<Item = &'a TodoItem>) -> bool {
    todos.into_iter().any(|todo| todo.stage.is_some())
}

/// Prefix of swarm plan item ids that stand for a whole lane of todos.
pub const LANE_ID_PREFIX: &str = "lane:";

/// Stable id of the lane a todo belongs to: `lane:<stage>:<group>` for a
/// grouped todo, or `lane:todo:<id>` for an ungrouped one (a lane of one).
pub fn lane_id(todo: &TodoItem) -> String {
    match lane_group(todo) {
        Some(group) => format!(
            "{LANE_ID_PREFIX}{}:{group}",
            todo.stage.map(|n| n.to_string()).unwrap_or_default()
        ),
        None => format!("{LANE_ID_PREFIX}todo:{}", todo.id),
    }
}

/// Lay out `todos` into stages and lanes. Every index appears exactly once.
pub fn stage_layout(todos: &[&TodoItem]) -> Vec<TodoStage> {
    let mut stages: Vec<TodoStage> = Vec::new();
    for (index, todo) in todos.iter().enumerate() {
        let stage_index = match todo.stage {
            Some(number) => match stages.iter().position(|s| s.number == Some(number)) {
                Some(found) => found,
                None => {
                    stages.push(TodoStage {
                        number: Some(number),
                        lanes: Vec::new(),
                    });
                    stages.len() - 1
                }
            },
            None => {
                stages.push(TodoStage {
                    number: None,
                    lanes: Vec::new(),
                });
                stages.len() - 1
            }
        };
        let stage = &mut stages[stage_index];
        let group = lane_group(todo);
        let existing = group.as_ref().and_then(|group| {
            stage
                .lanes
                .iter()
                .position(|l| l.group.as_ref() == Some(group))
        });
        match existing {
            Some(lane) => stage.lanes[lane].items.push(index),
            None => stage.lanes.push(TodoLane {
                group,
                items: vec![index],
            }),
        }
    }
    stages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(id: &str, stage: Option<u32>, group: Option<&str>) -> TodoItem {
        TodoItem {
            id: id.to_string(),
            content: id.to_string(),
            status: "pending".to_string(),
            priority: "high".to_string(),
            stage,
            group: group.map(str::to_string),
            ..Default::default()
        }
    }

    fn shape(todos: &[TodoItem]) -> Vec<Vec<Vec<usize>>> {
        let refs: Vec<&TodoItem> = todos.iter().collect();
        stage_layout(&refs)
            .into_iter()
            .map(|stage| stage.lanes.into_iter().map(|lane| lane.items).collect())
            .collect()
    }

    #[test]
    fn unstaged_list_is_sequential() {
        let todos = [todo("a", None, None), todo("b", None, Some("g"))];
        assert_eq!(shape(&todos), vec![vec![vec![0]], vec![vec![1]]]);
    }

    #[test]
    fn groups_become_lanes_and_stage_numbers_collect_across_the_list() {
        let todos = [
            todo("p1", Some(1), Some("parser")),
            todo("u1", Some(1), Some("ui")),
            todo("p2", Some(1), Some("parser")),
            todo("loose", Some(1), None),
            todo("i1", Some(2), Some("integration")),
            todo("d1", Some(1), Some("docs")),
        ];
        assert_eq!(
            shape(&todos),
            vec![vec![vec![0, 2], vec![1], vec![3], vec![5]], vec![vec![4]],]
        );
    }
}

/// A swarm lane owned by one worker: the coordinator's todos whose
/// [`lane_id`] is `lane` are mirrored into the worker's own todo list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaneLink {
    pub coordinator: String,
    pub worker: String,
    pub lane: String,
}

/// Seed (or refresh) a worker's todo list from the coordinator's lane: the
/// worker gets exactly the lane's todos, minus coordinator-only bookkeeping.
pub fn lane_todos_for_worker(coordinator_todos: &[TodoItem], lane: &str) -> Vec<TodoItem> {
    coordinator_todos
        .iter()
        .filter(|todo| lane_id(todo) == lane)
        .map(|todo| TodoItem {
            assigned_to: None,
            blocked_by: Vec::new(),
            ..todo.clone()
        })
        .collect()
}

/// Write a worker's current todo list back over its lane in the coordinator's
/// list. Last save wins: the worker's version replaces the lane wholesale, in
/// place, so items it added, split, reworded, or removed show up for the
/// coordinator. Every returned item is forced into the lane (same stage and
/// group) and tagged with the worker, so a worker can never move work into
/// another lane. Returns `None` when nothing changed.
pub fn merge_worker_lane(
    coordinator_todos: &[TodoItem],
    link: &LaneLink,
    worker_todos: &[TodoItem],
) -> Option<Vec<TodoItem>> {
    let first = coordinator_todos
        .iter()
        .position(|todo| lane_id(todo) == link.lane)?;
    let template = coordinator_todos[first].clone();
    let other_ids: std::collections::HashSet<&str> = coordinator_todos
        .iter()
        .filter(|todo| lane_id(todo) != link.lane)
        .map(|todo| todo.id.as_str())
        .collect();
    let lane: Vec<TodoItem> = worker_todos
        .iter()
        .map(|todo| {
            let mut item = todo.clone();
            item.stage = template.stage;
            item.group = template.group.clone();
            item.assigned_to = Some(link.worker.clone());
            if other_ids.contains(item.id.as_str()) {
                item.id = format!("{}:{}", link.worker, item.id);
            }
            item
        })
        .collect();
    // An ungrouped lane is keyed by its todo id; a worker that renamed or
    // removed it would orphan the link, so keep that single item stable.
    if template.group.is_none() && lane.iter().all(|todo| todo.id != template.id) {
        return None;
    }
    let mut merged = Vec::with_capacity(coordinator_todos.len() + lane.len());
    let mut inserted = false;
    for todo in coordinator_todos {
        if lane_id(todo) == link.lane {
            if !inserted {
                merged.extend(lane.iter().cloned());
                inserted = true;
            }
        } else {
            merged.push(todo.clone());
        }
    }
    (merged != coordinator_todos).then_some(merged)
}

#[cfg(test)]
mod lane_sync_tests {
    use super::*;

    fn item(id: &str, status: &str, stage: Option<u32>, group: Option<&str>) -> TodoItem {
        TodoItem {
            id: id.into(),
            content: format!("task {id}"),
            status: status.into(),
            priority: "high".into(),
            stage,
            group: group.map(str::to_string),
            ..Default::default()
        }
    }

    fn coordinator() -> Vec<TodoItem> {
        vec![
            item("p1", "pending", Some(1), Some("parser")),
            item("u1", "pending", Some(1), Some("ui")),
            item("p2", "pending", Some(1), Some("parser")),
            item("r1", "pending", Some(2), Some("release")),
        ]
    }

    fn link() -> LaneLink {
        LaneLink {
            coordinator: "boss".into(),
            worker: "fox".into(),
            lane: "lane:1:parser".into(),
        }
    }

    #[test]
    fn worker_is_seeded_with_exactly_its_lane() {
        let seeded = lane_todos_for_worker(&coordinator(), "lane:1:parser");
        let ids: Vec<&str> = seeded.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["p1", "p2"]);
        assert!(seeded.iter().all(|t| t.assigned_to.is_none()));
    }

    #[test]
    fn worker_edits_replace_the_lane_in_place() {
        let mut worker = lane_todos_for_worker(&coordinator(), "lane:1:parser");
        worker[0].status = "completed".into();
        worker[1].details = Some("touch src/lexer.rs".into());
        // Split a step and try to escape the lane: both get pinned back.
        let mut extra = item("p3", "pending", Some(9), Some("elsewhere"));
        extra.content = "add lexer tests".into();
        worker.push(extra);
        // Colliding id with another lane gets namespaced.
        worker.push(item("u1", "pending", None, None));
        let merged = merge_worker_lane(&coordinator(), &link(), &worker).unwrap();
        let ids: Vec<&str> = merged.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["p1", "p2", "p3", "fox:u1", "u1", "r1"]);
        for todo in &merged[..4] {
            assert_eq!(lane_id(todo), "lane:1:parser");
            assert_eq!(todo.assigned_to.as_deref(), Some("fox"));
        }
        assert_eq!(merged[0].status, "completed");
        assert_eq!(merged[1].details.as_deref(), Some("touch src/lexer.rs"));
        assert_eq!(merged[4].assigned_to, None, "other lanes untouched");
        // Same input again is a no-op.
        assert_eq!(merge_worker_lane(&merged, &link(), &worker), None);
    }

    #[test]
    fn removed_items_disappear_and_missing_lane_is_ignored() {
        let mut worker = lane_todos_for_worker(&coordinator(), "lane:1:parser");
        worker.remove(0);
        let merged = merge_worker_lane(&coordinator(), &link(), &worker).unwrap();
        assert!(merged.iter().all(|t| t.id != "p1"));
        let gone = LaneLink {
            lane: "lane:7:nothing".into(),
            ..link()
        };
        assert_eq!(merge_worker_lane(&coordinator(), &gone, &worker), None);
    }
}

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

#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed(String),
    Skipped,
}

#[derive(Debug, Clone)]
pub enum StepType {
    Command { command: String, args: Vec<String> },
    Script { content: String, interpreter: String },
    Http { url: String, method: String, body: Option<String> },
    Skill { skill_name: String, params: String },
    Subworkflow { workflow_name: String },
    Approval { message: String },
    Notification { message: String, channel: String },
    Condition { condition: String, if_true: Vec<WorkflowStep>, if_false: Vec<WorkflowStep> },
}

#[derive(Debug, Clone)]
pub struct WorkflowStep {
    pub name: String,
    pub description: String,
    pub step_type: StepType,
    pub depends_on: Vec<String>,
    pub timeout_secs: Option<u64>,
    pub retry_count: u32,
    pub allow_failure: bool,
    pub output_var: Option<String>,
}

impl WorkflowStep {
    pub fn new(name: &str, step_type: StepType) -> Self {
        WorkflowStep {
            name: name.to_string(),
            description: String::new(),
            step_type,
            depends_on: vec![],
            timeout_secs: None,
            retry_count: 0,
            allow_failure: false,
            output_var: None,
        }
    }

    pub fn cmd(name: &str, command: &str) -> Self {
        Self::new(name, StepType::Command {
            command: command.to_string(),
            args: vec![],
        })
    }

    pub fn script(name: &str, content: &str) -> Self {
        Self::new(name, StepType::Script {
            content: content.to_string(),
            interpreter: "powershell".to_string(),
        })
    }

    pub fn skill(name: &str, skill_name: &str, params: &str) -> Self {
        Self::new(name, StepType::Skill {
            skill_name: skill_name.to_string(),
            params: params.to_string(),
        })
    }

    pub fn approval(name: &str, message: &str) -> Self {
        Self::new(name, StepType::Approval {
            message: message.to_string(),
        })
    }
}
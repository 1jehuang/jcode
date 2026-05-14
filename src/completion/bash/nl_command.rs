use crate::completion::bash::{CompletionContext, CompletionKind, CompletionSuggestion};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

pub struct NlCommandGenerator {
    templates: IntentTemplateRegistry,
    history_index: CommandHistoryIndex,
    usage_stats: UsageFrequencyCache,
    config: NlCommandConfig,
}

#[derive(Debug, Clone)]
pub struct NlCommandConfig {
    pub max_candidates: usize,
    pub min_confidence: f64,
    pub enable_history_learning: bool,
    pub preferred_shell: ShellType,
    pub safety_level: SafetyLevel,
}

impl Default for NlCommandConfig {
    fn default() -> Self {
        Self {
            max_candidates: 5,
            min_confidence: 0.3,
            enable_history_learning: true,
            preferred_shell: ShellType::Bash,
            safety_level: SafetyLevel::Moderate,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShellType {
    Bash,
    Zsh,
    Fish,
    PowerShell,
}

impl std::fmt::Display for ShellType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellType::Bash => write!(f, "bash"),
            ShellType::Zsh => write!(f, "zsh"),
            ShellType::Fish => write!(f, "fish"),
            ShellType::PowerShell => write!(f, "powershell"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SafetyLevel {
    Conservative,
    Moderate,
    Expert,
}

#[derive(Debug, Clone)]
pub struct GeneratedCommand {
    pub command: String,
    pub explanation: String,
    pub confidence: f64,
    pub risk_level: RiskLevel,
    pub source: CommandSource,
    pub estimated_impact: ImpactEstimate,
    pub alternatives: Vec<String>,
}

impl GeneratedCommand {
    fn to_suggestion(&self) -> CompletionSuggestion {
        CompletionSuggestion {
            text: self.command.clone(),
            display_text: self.command.clone(),
            description: self.explanation.clone(),
            kind: CompletionKind::Snippet,
            priority: (self.confidence * 100.0) as u8,
            metadata: {
                let mut m = HashMap::new();
                m.insert("risk".into(), format!("{:?}", self.risk_level));
                m.insert("confidence".into(), format!("{:.2}", self.confidence));
                m
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RiskLevel {
    Safe,
    Moderate,
    Dangerous,
    Destructive,
}

impl RiskLevel {
    fn score(&self) -> u8 {
        match self {
            RiskLevel::Safe => 0,
            RiskLevel::Moderate => 1,
            RiskLevel::Dangerous => 2,
            RiskLevel::Destructive => 3,
        }
    }

    fn from_template(risk: &RiskLevel) -> Self {
        risk.clone()
    }
}

#[derive(Debug, Clone)]
pub enum CommandSource {
    Template { template_id: String },
    LlmGenerated { model: String, tokens_used: u32 },
    Historical { session_id: Uuid, similarity: f32 },
    CustomAlias { name: String },
}

#[derive(Debug, Clone)]
pub struct ImpactEstimate {
    pub files_affected: Option<usize>,
    pub network_access: bool,
    pub disk_write: bool,
    pub process_spawn: bool,
    pub estimated_duration_ms: Option<u64>,
}

impl Default for ImpactEstimate {
    fn default() -> Self {
        Self {
            files_affected: None,
            network_access: false,
            disk_write: false,
            process_spawn: false,
            estimated_duration_ms: None,
        }
    }
}

pub struct ClassifiedIntent {
    pub category: IntentCategory,
    pub confidence: f64,
    pub entities: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub enum IntentCategory {
    FileOperation,
    GitOperation,
    DockerOperation,
    NetworkRequest,
    ProcessManagement,
    PackageManagement,
    Search,
    Compression,
    SystemInfo,
    TextProcessing,
    Database,
    Other,
}

impl std::fmt::Display for IntentCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntentCategory::FileOperation => write!(f, "file"),
            IntentCategory::GitOperation => write!(f, "git"),
            IntentCategory::DockerOperation => write!(f, "docker"),
            IntentCategory::NetworkRequest => write!(f, "network"),
            IntentCategory::ProcessManagement => write!(f, "process"),
            IntentCategory::PackageManagement => write!(f, "package"),
            IntentCategory::Search => write!(f, "search"),
            IntentCategory::Compression => write!(f, "compression"),
            IntentCategory::SystemInfo => write!(f, "system"),
            IntentCategory::TextProcessing => write!(f, "text"),
            IntentCategory::Database => write!(f, "database"),
            IntentCategory::Other => write!(f, "other"),
        }
    }
}

pub struct IntentTemplate {
    id: String,
    patterns: Vec<String>,
    commands: Vec<TemplateCommand>,
    intent: IntentCategory,
    risk: RiskLevel,
    warning: Option<String>,
}

struct TemplateCommand {
    command: String,
    description: String,
    requires_confirmation: bool,
}

pub struct SafetyVerdict {
    pub is_safe: bool,
    pub risk_level: RiskLevel,
    pub warnings: Vec<String>,
    pub requires_user_approval: bool,
}

struct IntentTemplateRegistry {
    templates: Vec<IntentTemplate>,
}

struct CommandHistoryIndex {
    entries: Vec<HistoryEntry>,
}

struct HistoryEntry {
    input: String,
    command: String,
    category: IntentCategory,
    timestamp: DateTime<Utc>,
    frequency: u32,
}

struct UsageFrequencyCache {
    frequencies: HashMap<String, u32>,
}

impl Default for UsageFrequencyCache {
    fn default() -> Self {
        let mut freq = HashMap::new();
        freq.insert("git status".to_string(), 100);
        freq.insert("ls -la".to_string(), 80);
        freq.insert("docker ps".to_string(), 60);
        freq.insert("npm install".to_string(), 50);
        freq.insert("grep -r".to_string(), 40);
        Self { frequencies: freq }
    }
}

impl NlCommandGenerator {
    pub fn new(config: NlCommandConfig) -> Self {
        Self {
            templates: IntentTemplateRegistry::built_in(),
            history_index: CommandHistoryIndex::new(),
            usage_stats: UsageFrequencyCache::default(),
            config,
        }
    }

    pub fn generate(&self, input: &str, _ctx: &CompletionContext) -> Vec<GeneratedCommand> {
        let intent = self.classify_intent(input);
        let matched = self.match_templates(&intent, input);
        let mut ranked = self.rank_by_history(matched);
        ranked.retain(|c| c.confidence >= self.config.min_confidence);
        if ranked.len() > self.config.max_candidates {
            ranked.truncate(self.config.max_candidates);
        }
        ranked.into_iter()
            .filter(|c| self.validate_safety(c).is_safe)
            .collect()
    }

    pub fn classify_intent(&self, input: &str) -> ClassifiedIntent {
        let lower = input.to_lowercase();
        let entities = self.extract_entities(&lower);

        let (category, confidence) =
            if lower.contains("git") || lower.contains("commit") || lower.contains("branch")
                || lower.contains("merge") || lower.contains("push") || lower.contains("pull")
                || lower.contains("clone") || lower.contains("checkout") || lower.contains("stash")
                || lower.contains("rebase") || lower.contains("cherry") || lower.contains("diff")
                || lower.contains("reset") || lower.contains("revert")
            {
                (IntentCategory::GitOperation, 0.92)
            } else if lower.contains("docker") || lower.contains("container")
                || lower.contains("image") || lower.contains("compose") || lower.contains("volume")
            {
                (IntentCategory::DockerOperation, 0.90)
            } else if lower.contains("curl") || lower.contains("wget")
                || lower.contains("download") || lower.contains("http") || lower.contains("port")
                || lower.contains("ping") || lower.contains("ssh") || lower.contains("connect")
            {
                (IntentCategory::NetworkRequest, 0.88)
            } else if lower.contains("kill") || lower.contains("process")
                || lower.contains("cpu") || lower.contains("memory") || lower.contains("top")
                || lower.contains("ps ") || lower.contains("monitor") || lower.contains("htop")
            {
                (IntentCategory::ProcessManagement, 0.87)
            } else if lower.contains("install") || lower.contains("npm")
                || lower.contains("pip") || lower.contains("cargo") || lower.contains("brew")
                || lower.contains("apt") || lower.contains("yum") || lower.contains("update")
                || lower.contains("upgrade") || lower.contains("uninstall")
            {
                (IntentCategory::PackageManagement, 0.85)
            } else if lower.contains("find") || lower.contains("search")
                || lower.contains("grep") || lower.contains("locate") || lower.contains("look for")
            {
                (IntentCategory::Search, 0.84)
            } else if lower.contains("zip") || lower.contains("tar")
                || lower.contains("compress") || lower.contains("extract") || lower.contains("gzip")
                || lower.contains("archive")
            {
                (IntentCategory::Compression, 0.83)
            } else if lower.contains("system") || lower.contains("disk")
                || lower.contains("memory") || lower.contains("cpu info") || lower.contains("uname")
                || lower.contains("hostname") || lower.contains("uptime") || lower.contains("df ")
                || lower.contains("free ")
            {
                (IntentCategory::SystemInfo, 0.82)
            } else if lower.contains("sed") || lower.contains("awk")
                || lower.contains("sort") || lower.contains("uniq") || lower.contains("replace")
                || lower.contains("count") || lower.contains("lines") || lower.contains("word count")
                || lower.contains("cat ") || lower.contains("head") || lower.contains("tail")
                || lower.contains("wc ")
            {
                (IntentCategory::TextProcessing, 0.81)
            } else if lower.contains("sql") || lower.contains("database")
                || lower.contains("mysql") || lower.contains("postgres") || lower.contains("redis")
                || lower.contains("mongodb") || lower.contains("query")
            {
                (IntentCategory::Database, 0.80)
            } else if lower.contains("file") || lower.contains("copy")
                || lower.contains("move") || lower.contains("delete") || lower.contains("remove")
                || lower.contains("rename") || lower.contains("create") || lower.contains("chmod")
                || lower.contains("chown") || lower.contains("directory") || lower.contains("folder")
                || lower.contains("ls ") || lower.contains("list")
            {
                (IntentCategory::FileOperation, 0.86)
            } else {
                (IntentCategory::Other, 0.30)
            };

        ClassifiedIntent { category, confidence, entities }
    }

    pub fn match_templates(&self, intent: &ClassifiedIntent, input: &str) -> Vec<GeneratedCommand> {
        let lower = input.to_lowercase();
        let mut results = Vec::new();

        for template in &self.templates.templates {
            if template.intent != intent.category {
                continue;
            }
            for pattern in &template.patterns {
                if self.pattern_matches(pattern, &lower) {
                    if let Some(cmd) = &template.commands.first() {
                        let rendered = self.render_command(&cmd.command, &intent.entities);
                        results.push(GeneratedCommand {
                            command: rendered,
                            explanation: cmd.description.clone(),
                            confidence: intent.confidence
                                * self.pattern_relevance(pattern, &lower),
                            risk_level: RiskLevel::from_template(&template.risk),
                            source: CommandSource::Template {
                                template_id: template.id.clone(),
                            },
                            estimated_impact: self.estimate_impact(&cmd.command),
                            alternatives: template.commands.iter().skip(1).map(|c| c.command.clone()).collect(),
                        });
                    }
                    break;
                }
            }
        }
        results
    }

    fn pattern_matches(&self, pattern: &str, input: &str) -> bool {
        let keywords: Vec<&str> = pattern.split_whitespace().collect();
        keywords.iter().all(|kw| input.contains(kw))
    }

    fn pattern_relevance(&self, pattern: &str, input: &str) -> f64 {
        let keywords: Vec<&str> = pattern.split_whitespace().collect();
        let matches = keywords.iter().filter(|kw| input.contains(**kw)).count();
        if keywords.is_empty() { return 0.0; }
        matches as f64 / keywords.len() as f64
    }

    fn render_command(&self, tmpl: &str, entities: &HashMap<String, String>) -> String {
        let result = tmpl.replace("{file}", entities.get("file").map_or("", |s| s.as_str()))
            .replace("{name}", entities.get("name").map_or("", |s| s.as_str()))
            .replace("{ext}", entities.get("ext").map_or("", |s| s.as_str()))
            .replace("{port}", entities.get("port").map_or("", |s| s.as_str()))
            .replace("{branch}", entities.get("branch").map_or("", |s| s.as_str()))
            .replace("{url}", entities.get("url").map_or("", |s| s.as_str()))
            .replace("{pattern}", entities.get("pattern").map_or("", |s| s.as_str()))
            .replace("{pkg}", entities.get("package").map_or("", |s| s.as_str()))
            .replace("{container}", entities.get("container").map_or("", |s| s.as_str()))
            .replace("{image}", entities.get("image").map_or("", |s| s.as_str()));
        result
    }

    pub fn rank_by_history(&self, mut candidates: Vec<GeneratedCommand>) -> Vec<GeneratedCommand> {
        candidates.sort_by(|a, b| {
            let freq_a = self.usage_stats.frequencies.get(&a.command).copied().unwrap_or(0);
            let freq_b = self.usage_stats.frequencies.get(&b.command).copied().unwrap_or(0);
            freq_b.cmp(&freq_a)
                .then_with(|| b.confidence.partial_cmp(&a.confidence).unwrap_or(std::cmp::Ordering::Equal))
        });
        candidates
    }

    pub fn validate_safety(&self, cmd: &GeneratedCommand) -> SafetyVerdict {
        let warnings = Vec::new();
        let mut verdict = SafetyVerdict {
            is_safe: true,
            risk_level: cmd.risk_level.clone(),
            warnings,
            requires_user_approval: false,
        };
        match &cmd.risk_level {
            RiskLevel::Destructive => {
                verdict.is_safe = self.config.safety_level == SafetyLevel::Expert;
                verdict.requires_user_approval = true;
                verdict.warnings.push("This command may cause irreversible data loss.".into());
            }
            RiskLevel::Dangerous => {
                verdict.is_safe = self.config.safety_level != SafetyLevel::Conservative;
                verdict.requires_user_approval = true;
                verdict.warnings.push("This command may affect system state significantly.".into());
            }
            RiskLevel::Moderate => {
                verdict.requires_user_approval = self.config.safety_level == SafetyLevel::Conservative;
            }
            RiskLevel::Safe => {}
        }
        let dangerous_patterns = ["rm -rf /", "mkfs", "dd if=", "> /dev/sda", "chmod -R 777", ":(){ :|:& };:"];
        for pat in dangerous_patterns {
            if cmd.command.contains(pat) {
                verdict.is_safe = false;
                verdict.risk_level = RiskLevel::Destructive;
                verdict.requires_user_approval = true;
                verdict.warnings.push(format!("Blocked dangerous pattern: {}", pat));
            }
        }
        verdict
    }

    pub fn extract_entities(&self, input: &str) -> HashMap<String, String> {
        let mut entities = HashMap::new();
        let re_port = regex::Regex::new(r"(?i)(?:port\s*[:=]?\s*)(\d{1,5})").ok();
        let re_url = regex::Regex::new(r"https?://[^\s]+").ok();
        let re_file_ext = regex::Regex::new(r"\.(\w{1,10})\b").ok();
        let re_quoted = regex::Regex::new(r#""([^"]+)""#).ok();

        if let Some(re) = &re_port {
            if let Some(cap) = re.captures(input) {
                entities.insert("port".into(), cap[1].to_string());
            }
        }
        if let Some(re) = &re_url {
            if let Some(cap) = re.captures(input) {
                entities.insert("url".into(), cap[0].to_string());
            }
        }
        if let Some(re) = &re_file_ext {
            if let Some(cap) = re.captures(input) {
                entities.insert("ext".into(), cap[1].to_string());
            }
        }
        if let Some(re) = &re_quoted {
            if let Some(cap) = re.captures(input) {
                entities.insert("name".into(), cap[1].to_string());
                entities.insert("file".into(), cap[1].to_string());
            }
        }
        let pkg_keywords = [("npm ", "package"), ("pip install ", "package"), ("cargo add ", "package")];
        for (prefix, key) in &pkg_keywords {
            if let Some(rest) = input.strip_prefix(prefix) {
                let name = rest.trim().split_whitespace().next().unwrap_or("");
                if !name.is_empty() {
                    entities.insert(key.to_string(), name.to_string());
                }
            }
        }
        entities
    }

    fn estimate_impact(&self, cmd: &str) -> ImpactEstimate {
        ImpactEstimate {
            files_affected: if cmd.contains("*") || cmd.contains("-r") { Some(99) } else { None },
            network_access: cmd.contains("curl") || cmd.contains("wget") || cmd.contains("ssh"),
            disk_write: cmd.contains(">") || cmd.contains("mv ") || cmd.contains("cp ")
                || cmd.contains("mkdir") || cmd.contains("touch "),
            process_spawn: cmd.starts_with("sudo") || cmd.contains("systemctl")
                || cmd.contains("service ") || cmd.contains("nohup"),
            estimated_duration_ms: if cmd.contains("build") || cmd.contains("compile") {
                Some(30000)
            } else {
                None
            },
        }
    }
}

impl IntentTemplateRegistry {
    fn built_in() -> Self {
        let templates = vec![
            t!("file_list", ["list files", "show files", "ls", "what files"], [c("ls -la", "List all files with details", false)], FileOperation, Safe, None),
            t!("file_find_name", ["find file by name", "search for file", "where is file", "find file named"], [c("find . -name '{name}'", "Find file by name recursively", false), c("fd '{name}'", "Fast find using fd", false)], FileOperation, Safe, None),
            t!("file_find_type", ["find all rust files", "find all js files", "find files with extension"], [c("find . -type f -name '*.{ext}'", "Find files by extension", false), c("fd '.{ext}'", "Fast find by extension", false)], FileOperation, Safe, None),
            t!("file_count_lines", ["count lines in file", "how many lines", "line count", "wc lines"], [c("wc -l {file}", "Count lines in file", false)], TextProcessing, Safe, None),
            t!("file_copy", ["copy file", "duplicate file", "cp file"], [c("cp {file} {file}.bak", "Copy file with backup extension", false)], FileOperation, Moderate, None),
            t!("file_move", ["move file", "rename file", "mv file"], [c("mv {file} ./new_location/", "Move file to location", false)], FileOperation, Moderate, None),
            t!("file_delete", ["delete file", "remove file", "rm file"], [c("rm {file}", "Remove a single file", false)], FileOperation, Moderate, Some("File deletion cannot be undone".into())),
            t!("file_delete_recursive", ["delete folder", "remove directory", "rm directory", "clean folder"], [c("rm -rf {file}", "Recursively remove directory", true)], FileOperation, Dangerous, Some("Irrecoverable deletion of directory and contents".into())),
            t!("file_chmod", ["change permissions", "make executable", "chmod", "set permissions"], [c("chmod +x {file}", "Make file executable", false), c("chmod 644 {file}", "Set read-write permissions", false)], FileOperation, Moderate, None),
            t!("file_disk_usage", ["disk usage", "du folder size", "folder size", "how big is folder"], [c("du -sh * | sort -hr", "Show disk usage sorted by size", false), c("du -sh {file}", "Show size of specific path", false)], SystemInfo, Safe, None),
            t!("compress_images", ["compress images", "optimize images", "resize images", "shrink images"], [c("mogrify -quality 85 -resize 50% *.jpg", "Compress JPEG images using ImageMagick", false), c("for f in *.png; do optipng \"$f\"; done", "Optimize PNG files losslessly", false)], Compression, Moderate, None),
            t!("compress_tar_gz", ["create tar.gz", "compress to tar", "archive folder", "tar gz"], [c("tar -czvf archive.tar.gz {file}/", "Create compressed tar archive", false)], Compression, Safe, None),
            t!("extract_tar_gz", ["extract tar.gz", "untar file", "unzip tar", "decompress archive"], [c("tar -xzvf {file}", "Extract tar.gz archive", false)], Compression, Safe, None),
            t!("compress_zip", ["zip file", "create zip", "compress to zip"], [c("zip -r archive.zip {file}/", "Create ZIP archive", false)], Compression, Safe, None),
            t!("extract_zip", ["unzip file", "extract zip"], [c("unzip {file}", "Extract ZIP archive", false)], Compression, Safe, None),
            t!("git_status", ["git status", "check git status", "what changed", "git changes"], [c("git status", "Show working tree status", false)], GitOperation, Safe, None),
            t!("git_diff", ["show diff", "git diff", "what changed in detail", "see changes"], [c("git diff", "Show unstaged changes", false), c("git diff --staged", "Show staged changes", false)], GitOperation, Safe, None),
            t!("git_add_all", ["add all changes", "stage everything", "git add all", "track all files"], [c("git add .", "Stage all changes", false)], GitOperation, Safe, None),
            t!("git_commit", ["commit changes", "save changes", "git commit"], [c("git commit -m 'message'", "Commit staged changes with message", false)], GitOperation, Safe, None),
            t!("git_commit_amend", ["amend commit", "fix last commit message", "edit commit message"], [c("git commit --amend -m 'new message'", "Amend last commit message", true)], GitOperation, Moderate, Some("Rewrites git history".into())),
            t!("git_undo_commit", ["undo last commit", "revert last commit", "cancel commit", "remove last commit"], [c("git reset --soft HEAD~1", "Undo last commit but keep changes staged", true), c("git reset HEAD~1", "Undo last commit and unstage", true)], GitOperation, Moderate, Some("Removes the most recent commit from history".into())),
            t!("git_push", ["push to remote", "upload changes", "git push", "send to remote"], [c("git push origin HEAD", "Push current branch to remote", false), c("git push", "Push to tracked remote branch", false)], GitOperation, Safe, None),
            t!("git_pull", ["pull latest", "update from remote", "git pull", "fetch changes"], [c("git pull", "Pull and merge remote changes", false), c("git pull --rebase", "Pull and rebase on top of remote", false)], GitOperation, Safe, None),
            t!("git_create_branch", ["create branch", "new branch", "make branch", "git branch create"], [c("git checkout -b {branch}", "Create and switch to new branch", false), c("git branch {branch}", "Create new branch without switching", false)], GitOperation, Safe, None),
            t!("git_switch_branch", ["switch branch", "change branch", "checkout branch", "go to branch"], [c("git checkout {branch}", "Switch to specified branch", false), c("git switch {branch}", "Switch branch (modern syntax)", false)], GitOperation, Safe, None),
            t!("git_merge_branch", ["merge branch", "combine branches", "git merge"], [c("git merge {branch}", "Merge specified branch into current", true)], GitOperation, Moderate, Some("Merge may introduce conflicts requiring resolution".into())),
            t!("git_delete_branch", ["delete branch", "remove branch", "git branch delete"], [c("git branch -d {branch}", "Delete merged branch locally", true), c("git branch -D {branch}", "Force delete branch (even if unmerged)", true)], GitOperation, Moderate, None),
            t!("git_stash", ["stash changes", "save changes temporarily", "git stash", "hide changes"], [c("git stash", "Stash current changes", false), c("git stash push -m 'description'", "Stash with descriptive message", false)], GitOperation, Safe, None),
            t!("git_stash_pop", ["restore stash", "pop stash", "apply stashed changes"], [c("git stash pop", "Apply and remove most recent stash", false)], GitOperation, Safe, None),
            t!("git_log", ["show log", "git log", "view history", "commit history"], [c("git log --oneline -20", "Show recent 20 commits", false), c("git log --graph --oneline --all", "Show graph of all branches", false)], GitOperation, Safe, None),
            t!("git_remote_url", ["remote url", "git remote", "repo url", "where is repo"], [c("git remote -v", "Show remote URLs", false)], GitOperation, Safe, None),
            t!("git_clone", ["clone repo", "git clone", "download repository"], [c("git clone {url}", "Clone repository from URL", false)], GitOperation, Safe, None),
            t!("git_show_file_history", ["show file history", "blame file", "who changed this line", "git blame"], [c("git blame {file}", "Show who modified each line", false), c("git log -p --follow {file}", "Show full change history of file", false)], GitOperation, Safe, None),
            t!("git_clean_untracked", ["clean untracked", "remove untracked files", "git clean"], [c("git clean -fd", "Remove untracked files and directories", true), c("git clean -nfd", "Preview what would be cleaned (dry run)", false)], GitOperation, Dangerous, Some("Permanently removes untracked files".into())),
            t!("docker_ps", ["running containers", "list containers", "docker ps", "show containers"], [c("docker ps", "List running containers", false), c("docker ps -a", "List all containers including stopped", false)], DockerOperation, Safe, None),
            t!("docker_images", ["list images", "docker images", "show images"], [c("docker images", "List Docker images", false)], DockerOperation, Safe, None),
            t!("docker_stop_all", ["stop all containers", "stop docker", "shutdown containers"], [c("docker stop $(docker ps -q)", "Stop all running containers", true)], DockerOperation, Moderate, Some("Stops all running containers immediately".into())),
            t!("docker_remove_stopped", ["remove stopped containers", "clean stopped docker", "prune containers"], [c("docker container prune -f", "Remove all stopped containers", true)], DockerOperation, Moderate, None),
            t!("docker_cleanup", ["clean up docker", "docker cleanup", "free docker space"], [c("docker system prune -af", "Remove all unused Docker data (images, networks, volumes)", true)], DockerOperation, Dangerous, Some("Removes unused images, networks, build cache, and optionally volumes".into())),
            t!("docker_build", ["build image", "docker build", "create image"], [c("docker build -t {image} .", "Build Docker image from Dockerfile", false)], DockerOperation, Safe, None),
            t!("docker_run", ["run container", "start container", "docker run"], [c("docker run -it {image} bash", "Run container interactively", false), c("docker run -d -p {port}:80 {image}", "Run container in background with port mapping", false)], DockerOperation, Moderate, None),
            t!("docker_logs", ["container logs", "docker logs", "show logs"], [c("docker logs {container}", "Show container logs", false), c("docker logs -f {container}", "Follow container logs in real-time", false)], DockerOperation, Safe, None),
            t!("docker_exec", ["exec into container", "enter container", "docker exec", "shell into container"], [c("docker exec -it {container} bash", "Execute bash inside running container", false)], DockerOperation, Safe, None),
            t!("docker_compose_up", ["compose up", "docker compose start", "start services"], [c("docker compose up -d", "Start services in detached mode", false)], DockerOperation, Safe, None),
            t!("docker_compose_down", ["compose down", "docker compose stop", "stop services"], [c("docker compose down", "Stop and remove containers/networks", true)], DockerOperation, Moderate, None),
            t!("curl_get", ["curl request", "http get", "fetch url", "api request"], [c("curl -s {url}", "Make HTTP GET request", false), c("curl -s {url} | jq .", "Fetch and pretty-print JSON response", false)], NetworkRequest, Safe, None),
            t!("curl_post", ["post request", "send post", "http post", "api post"], [c("curl -X POST -H 'Content-Type: application/json' -d '{{}}' {url}", "Send JSON POST request", false)], NetworkRequest, Safe, None),
            t!("download_file", ["download file", "wget download", "save from url"], [c("curl -LO {url}", "Download file from URL", false), c("wget {url}", "Download file using wget", false)], NetworkRequest, Safe, None),
            t!("check_port", ["check port", "port open", "is port listening", "port in use"], [c("netstat -tlnp | grep :{port}", "Check if port is listening", false), c("ss -tlnp | grep :{port}", "Check port using ss", false), c("lsof -i :{port}", "Show what's using the port", false)], NetworkRequest, Safe, None),
            t!("ping_test", ["ping server", "test connection", "network test", "can i reach"], [c("ping -c 4 google.com", "Test network connectivity (4 packets)", false)], NetworkRequest, Safe, None),
            t!("kill_process_port", ["kill process on port", "free up port", "kill port", "port occupied"], [c("kill $(lsof -t -i :{port})", "Kill process listening on port", true), c("fuser -k {port}/tcp", "Kill process on port using fuser", true)], ProcessManagement, Dangerous, Some("Terminates whatever process is using this port".into())),
            t!("find_process", ["find process", "search process", "find running", "ps search"], [c("ps aux | grep {pattern}", "Find processes matching pattern", false), c("pgrep -a {pattern}", "Find process IDs matching pattern", false)], ProcessManagement, Safe, None),
            t!("monitor_cpu", ["monitor cpu", "cpu usage", "top process", "htop"], [c("htop", "Interactive process viewer (recommended)", false), c("top -o %CPU", "Show processes sorted by CPU usage", false)], ProcessManagement, Safe, None),
            t!("monitor_memory", ["memory usage", "ram usage", "free memory", "how much memory"], [c("free -h", "Show memory usage in human-readable format", false), c("vmstat 1 5", "Monitor memory stats every second", false)], ProcessManagement, Safe, None),
            t!("background_process", ["run background", "nohup run", "detach process", "daemonize"], [c("nohup {command} > output.log 2>&1 &", "Run command in background persistently", false)], ProcessManagement, Moderate, None),
            t!("npm_install", ["install package", "npm install", "add dependency"], [c("npm install {package}", "Install npm package and save to dependencies", false), c("npm install -D {package}", "Install as dev dependency", false)], PackageManagement, Safe, None),
            t!("npm_update", ["update packages", "npm update", "upgrade deps"], [c("npm update", "Update all packages per semver ranges", false)], PackageManagement, Safe, None),
            t!("npm_run_script", ["run script", "npm run", "execute script"], [c("npm run {command}", "Run npm script defined in package.json", false)], PackageManagement, Safe, None),
            t!("cargo_build", ["build project", "cargo build", "compile project", "rust build"], [c("cargo build", "Build Rust project in debug mode", false), c("cargo build --release", "Build optimized release binary", false)], PackageManagement, Safe, None),
            t!("cargo_test", ["run tests", "cargo test", "rust tests"], [c("cargo test", "Run all Rust tests", false), c("cargo test --release", "Run tests in release mode", false)], PackageManagement, Safe, None),
            t!("cargo_check", ["check code", "cargo check", "quick compile check"], [c("cargo check", "Quick type-check without producing binary", false)], PackageManagement, Safe, None),
            t!("pip_install", ["install python package", "pip install", "python dependency"], [c("pip install {package}", "Install Python package", false), c("pip install -r requirements.txt", "Install from requirements file", false)], PackageManagement, Safe, None),
            t!("brew_install", ["homebrew install", "brew install", "macos package"], [c("brew install {package}", "Install package via Homebrew", false)], PackageManagement, Safe, None),
            t!("apt_install", ["apt install", "ubuntu install", "debian package"], [c("sudo apt install {package}", "Install package via apt (Debian/Ubuntu)", false)], PackageManagement, Moderate, None),
            t!("grep_search", ["grep search", "search in files", "find text", "search content"], [c("grep -rn '{pattern}' .", "Recursively search for pattern in files", false), c("rg '{pattern}'", "Search using ripgrep (faster alternative)", false)], Search, Safe, None),
            t!("grep_ignore_case", ["case insensitive search", "grep ignore case", "search ignoring case"], [c("grep -rni '{pattern}' .", "Case-insensitive recursive search", false)], Search, Safe, None),
            t!("sed_replace", ["replace text", "sed replace", "find and replace", "substitute text"], [c("sed -i 's/{pattern}/replacement/g' {file}", "Replace text in file (in-place)", true)], TextProcessing, Moderate, Some("Modifies file in-place - consider making a backup first".into())),
            t!("sort_file", ["sort lines", "sort file", "order lines"], [c("sort {file}", "Sort file contents alphabetically", false), c("sort -u {file}", "Sort and remove duplicates", false)], TextProcessing, Safe, None),
            t!("tail_follow", ["follow log", "tail file", "watch file", "live log"], [c("tail -f {file}", "Follow file updates in real-time", false)], TextProcessing, Safe, None),
            t!("head_lines", ["show first lines", "head file", "beginning of file"], [c("head -n 50 {file}", "Show first 50 lines of file", false)], TextProcessing, Safe, None),
            t!("cat_file", ["show file content", "read file", "display file", "print file"], [c("cat {file}", "Display file contents", false), c("bat {file}", "Display file with syntax highlighting", false)], TextProcessing, Safe, None),
            t!("system_uptime", ["uptime", "how long running", "system uptime", "since when"], [c("uptime", "Show system uptime and load averages", false)], SystemInfo, Safe, None),
            t!("system_info", ["system info", "os version", "uname", "system details"], [c("uname -a", "Show complete system information", false), c("cat /etc/os-release", "Show OS release information", false)], SystemInfo, Safe, None),
            t!("disk_free", ["disk space", "free disk", "df", "available space"], [c("df -h", "Show disk space usage in human-readable format", false)], SystemInfo, Safe, None),
            t!("env_vars", ["environment variables", "env", "print env", "show env vars"], [c("env | sort", "Print sorted environment variables", false), c("printenv", "Print all environment variables", false)], SystemInfo, Safe, None),
            t!("which_command", ["which command", "where is command", "command location", "binary path"], [c("which {command}", "Show path to command executable", false), c("type {command}", "Show command type and location", false)], SystemInfo, Safe, None),
            t!("ssh_connect", ["ssh connect", "remote login", "ssh into server", "connect remote"], [c("ssh user@host", "Connect to remote host via SSH", false)], NetworkRequest, Safe, None),
            t!("scp_copy_remote", ["scp copy", "copy to server", "scp upload", "transfer file"], [c("scp {file} user@host:/path/", "Copy file to remote host via SCP", false)], NetworkRequest, Safe, None),
            t!("watch_command", ["watch command", "repeat command", "periodic run", "every 2 seconds"], [c("watch -n 2 '{command}', "Repeat command every 2 seconds", false)], ProcessManagement, Safe, None),
            t!("chmod_recursive", ["chmod recursive", "permissions recursive", "set permissions folder"], [c("chmod -R 755 {file}", "Set permissions recursively on directory", true)], FileOperation, Moderate, None),
            t!("find_large_files", ["large files", "big files", "find biggest", "disk hogs"], [c("find . -type f -size +100M -exec ls -lh {} \\; | sort -k5 -hr", "Find files larger than 100MB sorted by size", false)], FileOperation, Safe, None),
            t!("git_reset_hard", ["hard reset", "discard all changes", "git reset hard", "throw away changes"], [c("git reset --hard HEAD", "Discard all working directory changes", true)], GitOperation, Dangerous, Some("All uncommitted changes will be permanently lost!".into())),
            t!("force_push", ["force push", "overwrite remote", "git force push"], [c("git push --force-with-lease origin HEAD", "Force push with lease safety check", true)], GitOperation, Dangerous, Some("Overwrites remote history - use only when necessary".into())),
        ];
        Self { templates }
    }
}

impl CommandHistoryIndex {
    fn new() -> Self {
        Self { entries: Vec::new() }
    }

    fn find_similar(&self, _input: &str) -> Option<(Uuid, f32)> {
        None
    }
}

fn t!(
    id: &str,
    patterns: [&str; 4],
    commands: [(&str, &str, bool); 1],
    category: IntentCategory,
    risk: RiskLevel,
    warning: Option<String>
) -> IntentTemplate {
    IntentTemplate {
        id: id.to_string(),
        patterns: patterns.iter().map(|p| p.to_string()).collect(),
        commands: commands.iter().map(|(cmd, desc, confirm)| TemplateCommand {
            command: cmd.to_string(),
            description: desc.to_string(),
            requires_confirmation: *confirm,
        }).collect(),
        intent: category,
        risk,
        warning,
    }
}

fn t_multi!(
    id: &str,
    patterns: [&str; 4],
    commands: &[(&str, &str, bool)],
    category: IntentCategory,
    risk: RiskLevel,
    warning: Option<String>
) -> IntentTemplate {
    IntentTemplate {
        id: id.to_string(),
        patterns: patterns.iter().map(|p| p.to_string()).collect(),
        commands: commands.iter().map(|(cmd, desc, confirm)| TemplateCommand {
            command: cmd.to_string(),
            description: desc.to_string(),
            requires_confirmation: *confirm,
        }).collect(),
        intent: category,
        risk,
        warning,
    }
}

fn c(cmd: &str, desc: &str, confirm: bool) -> (&str, &str, bool) {
    (cmd, desc, confirm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_generator() -> NlCommandGenerator {
        NlCommandGenerator::new(NlCommandConfig::default())
    }

    fn make_ctx(line: &str) -> CompletionContext {
        CompletionContext::new(line, line.len())
    }

    #[test]
    fn test_classify_intent_git_operation() {
        let gen = make_generator();
        let intent = gen.classify_intent("undo last commit");
        assert_eq!(intent.category, IntentCategory::GitOperation);
        assert!(intent.confidence > 0.8);
    }

    #[test]
    fn test_classify_intent_docker() {
        let gen = make_generator();
        let intent = gen.classify_intent("stop all containers");
        assert_eq!(intent.category, IntentCategory::DockerOperation);
    }

    #[test]
    fn test_classify_intent_network() {
        let gen = make_generator();
        let intent = gen.classify_intent("curl request to api");
        assert_eq!(intent.category, IntentCategory::NetworkRequest);
    }

    #[test]
    fn test_classify_intent_process() {
        let gen = make_generator();
        let intent = gen.classify_intent("kill process on port 3000");
        assert_eq!(intent.category, IntentCategory::ProcessManagement);
    }

    #[test]
    fn test_classify_intent_package() {
        let gen = make_generator();
        let intent = gen.classify_intent("install lodash npm");
        assert_eq!(intent.category, IntentCategory::PackageManagement);
    }

    #[test]
    fn test_classify_intent_search() {
        let gen = make_generator();
        let intent = gen.classify_intent("find all occurrences of error");
        assert_eq!(intent.category, IntentCategory::Search);
    }

    #[test]
    fn test_classify_intent_compression() {
        let gen = make_generator();
        let intent = gen.classify_intent("compress folder to zip");
        assert_eq!(intent.category, IntentCategory::Compression);
    }

    #[test]
    fn test_classify_intent_system_info() {
        let gen = make_generator();
        let intent = gen.classify_intent("show system uptime");
        assert_eq!(intent.category, IntentCategory::SystemInfo);
    }

    #[test]
    fn test_classify_intent_text_processing() {
        let gen = make_generator();
        let intent = gen.classify_intent("count lines in main.rs");
        assert_eq!(intent.category, IntentCategory::TextProcessing);
    }

    #[test]
    fn test_classify_intent_database() {
        let gen = make_generator();
        let intent = gen.classify_intent("query mysql database");
        assert_eq!(intent.category, IntentCategory::Database);
    }

    #[test]
    fn test_classify_intent_file_operation() {
        let gen = make_generator();
        let intent = gen.classify_intent("delete old logs");
        assert_eq!(intent.category, IntentCategory::FileOperation);
    }

    #[test]
    fn test_classify_intent_other() {
        let gen = make_generator();
        let intent = gen.classify_intent("hello world");
        assert_eq!(intent.category, IntentCategory::Other);
        assert!(intent.confidence < 0.5);
    }

    #[test]
    fn test_generate_git_status() {
        let gen = make_generator();
        let ctx = make_ctx("git status");
        let results = gen.generate("git status", &ctx);
        assert!(!results.is_empty());
        assert!(results[0].command.contains("git status"));
    }

    #[test]
    fn test_generate_docker_stop_all() {
        let gen = make_generator();
        let ctx = make_ctx("stop all containers");
        let results = gen.generate("stop all containers", &ctx);
        assert!(!results.is_empty());
        assert!(results[0].command.contains("docker stop"));
    }

    #[test]
    fn test_generate_curl_request() {
        let gen = make_generator();
        let ctx = make_ctx("curl request to https://api.example.com");
        let results = gen.generate("curl request to https://api.example.com", &ctx);
        assert!(!results.is_empty());
        assert!(results[0].command.contains("curl"));
    }

    #[test]
    fn test_extract_entities_port() {
        let gen = make_generator();
        let entities = gen.extract_entities("kill process on port 8080");
        assert_eq!(entities.get("port").map(|s| s.as_str()), Some("8080"));
    }

    #[test]
    fn test_extract_entities_url() {
        let gen = make_generator();
        let entities = gen.extract_entities("download https://example.com/file.zip");
        assert_eq!(entities.get("url").map(|s| s.contains("example.com")), Some(true));
    }

    #[test]
    fn test_extract_entities_quoted_name() {
        let gen = make_generator();
        let entities = gen.extract_entities(r#"find file named "config.toml""#);
        assert_eq!(entities.get("name").map(|s| s.as_str()), Some("config.toml"));
    }

    #[test]
    fn test_validate_safe_command() {
        let gen = make_generator();
        let cmd = GeneratedCommand {
            command: "ls -la".into(),
            explanation: "list files".into(),
            confidence: 0.9,
            risk_level: RiskLevel::Safe,
            source: CommandSource::Template { template_id: "test".into() },
            estimated_impact: ImpactEstimate::default(),
            alternatives: vec![],
        };
        let verdict = gen.validate_safety(&cmd);
        assert!(verdict.is_safe);
        assert!(!verdict.requires_user_approval);
    }

    #[test]
    fn test_validate_destructive_blocked_conservative() {
        let config = NlCommandConfig { safety_level: SafetyLevel::Conservative, ..Default::default() };
        let gen = NlCommandGenerator::new(config);
        let cmd = GeneratedCommand {
            command: "rm -rf /tmp/old".into(),
            explanation: "delete".into(),
            confidence: 0.9,
            risk_level: RiskLevel::Destructive,
            source: CommandSource::Template { template_id: "test".into() },
            estimated_impact: ImpactEstimate::default(),
            alternatives: vec![],
        };
        let verdict = gen.validate_safety(&cmd);
        assert!(!verdict.is_safe);
        assert!(verdict.requires_user_approval);
    }

    #[test]
    fn test_validate_destructive_allowed_expert() {
        let config = NlCommandConfig { safety_level: SafetyLevel::Expert, ..Default::default() };
        let gen = NlCommandGenerator::new(config);
        let cmd = GeneratedCommand {
            command: "rm -rf /tmp/old".into(),
            explanation: "delete".into(),
            confidence: 0.9,
            risk_level: RiskLevel::Destructive,
            source: CommandSource::Template { template_id: "test".into() },
            estimated_impact: ImpactEstimate::default(),
            alternatives: vec![],
        };
        let verdict = gen.validate_safety(&cmd);
        assert!(verdict.is_safe);
    }

    #[test]
    fn test_block_dangerous_pattern_rm_rf_root() {
        let gen = make_generator();
        let cmd = GeneratedCommand {
            command: "rm -rf /".into(),
            explanation: "bad".into(),
            confidence: 0.9,
            risk_level: RiskLevel::Safe,
            source: CommandSource::Template { template_id: "test".into() },
            estimated_impact: ImpactEstimate::default(),
            alternatives: vec![],
        };
        let verdict = gen.validate_safety(&cmd);
        assert!(!verdict.is_safe);
        assert_eq!(verdict.risk_level, RiskLevel::Destructive);
    }

    #[test]
    fn test_rank_by_history_prefers_frequent() {
        let gen = make_generator();
        let mut cmds = vec![
            GeneratedCommand {
                command: "git status".into(),
                explanation: "".into(),
                confidence: 0.8,
                risk_level: RiskLevel::Safe,
                source: CommandSource::Template { template_id: "a".into() },
                estimated_impact: ImpactEstimate::default(),
                alternatives: vec![],
            },
            GeneratedCommand {
                command: "rare_command_xyz".into(),
                explanation: "".into(),
                confidence: 0.95,
                risk_level: RiskLevel::Safe,
                source: CommandSource::Template { template_id: "b".into() },
                estimated_impact: ImpactEstimate::default(),
                alternatives: vec![],
            },
        ];
        let ranked = gen.rank_by_history(cmds);
        assert_eq!(ranked[0].command, "git status");
    }

    #[test]
    fn test_generated_to_suggestion_conversion() {
        let cmd = GeneratedCommand {
            command: "git log --oneline".into(),
            explanation: "Show recent commits".into(),
            confidence: 0.92,
            risk_level: RiskLevel::Safe,
            source: CommandSource::Template { template_id: "git_log".into() },
            estimated_impact: ImpactEstimate::default(),
            alternatives: vec!["git log --graph".into()],
        };
        let sug = cmd.to_suggestion();
        assert_eq!(sug.text, "git log --oneline");
        assert_eq!(sug.kind, CompletionKind::Snippet);
        assert_eq!(sug.priority, 92);
    }

    #[test]
    fn test_render_command_with_entities() {
        let gen = make_generator();
        let mut entities = HashMap::new();
        entities.insert("port".into(), "3000".into());
        let result = gen.render_command("kill $(lsof -t -i :{port})", &entities);
        assert!(result.contains("3000"));
    }

    #[test]
    fn test_risk_level_score_ordering() {
        assert!(RiskLevel::Safe.score() < RiskLevel::Moderate.score());
        assert!(RiskLevel::Moderate.score() < RiskLevel::Dangerous.score());
        assert!(RiskLevel::Dangerous.score() < RiskLevel::Destructive.score());
    }

    #[test]
    fn test_config_default_values() {
        let cfg = NlCommandConfig::default();
        assert_eq!(cfg.max_candidates, 5);
        assert_eq!(cfg.min_confidence, 0.3);
        assert!(cfg.enable_history_learning);
        assert_eq!(cfg.preferred_shell, ShellType::Bash);
        assert_eq!(cfg.safety_level, SafetyLevel::Moderate);
    }

    #[test]
    fn test_shell_type_display() {
        assert_eq!(ShellType::Bash.to_string(), "bash");
        assert_eq!(ShellType::PowerShell.to_string(), "powershell");
    }

    #[test]
    fn test_intent_category_display() {
        assert_eq!(IntentCategory::GitOperation.to_string(), "git");
        assert_eq!(IntentCategory::DockerOperation.to_string(), "docker");
        assert_eq!(IntentCategory::FileOperation.to_string(), "file");
    }

    #[test]
    fn test_estimate_impact_network_cmd() {
        let gen = make_generator();
        let impact = gen.estimate_impact("curl https://api.example.com");
        assert!(impact.network_access);
        assert!(!impact.disk_write);
    }

    #[test]
    fn test_estimate_impact_build_cmd() {
        let gen = make_generator();
        let impact = gen.estimate_impact("cargo build --release");
        assert!(impact.process_spawn);
        assert_eq!(impact.estimated_duration_ms, Some(30000));
    }

    #[test]
    fn test_template_registry_has_templates() {
        let registry = IntentTemplateRegistry::built_in();
        assert!(registry.templates.len() >= 50);
    }

    #[test]
    fn test_generate_npm_install() {
        let gen = make_generator();
        let ctx = make_ctx("install lodash");
        let results = gen.generate("install lodash", &ctx);
        assert!(!results.is_empty());
        assert!(results[0].command.contains("npm") || results[0].command.contains("install"));
    }
}

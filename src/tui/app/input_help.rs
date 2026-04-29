use super::*;

impl App {
    pub(super) fn command_help(&self, topic: &str) -> Option<String> {
        let topic = topic.trim().trim_start_matches('/').to_lowercase();
        let help = match topic.as_str() {
            "help" | "commands" => {
                "`/help`\nShow general command list and keyboard shortcuts.\n\n`/help <command>`\nShow detailed help for one command."
            }
            "compact" => {
                "`/compact`\nForce context compaction now.\nStarts background summarization and applies it automatically when ready.\n\n`/compact mode`\nShow current compaction mode for this session.\n\n`/compact mode <reactive|proactive|semantic>`\nChange compaction mode for this session."
            }
            "cache" => {
                "`/cache stats`\nShow KV cache stats for this session: cache read/write totals, hit ratios, current baseline, and recent miss attributions.\n\n`/cache`\nToggle Anthropic cache TTL between 5 minutes and 1 hour.\n\n`/cache 1h` / `/cache 5m`\nSet Anthropic cache TTL explicitly."
            }
            "fix" => {
                "`/fix`\nRun recovery actions when the model cannot continue.\nRepairs missing tool outputs, resets provider session state, and starts compaction when possible."
            }
            "rewind" => {
                "`/rewind`\nShow numbered conversation history.\n\n`/rewind N`\nRewind to message N (drops everything after it and resets provider session)."
            }
            "clear" => {
                "`/clear`\nClear current conversation, queue, and display; starts a fresh session."
            }
            "model" => {
                "`/model`\nOpen model picker.\n\n`/model <name>`\nSwitch model.\n\n`/model <name>@<provider>`\nPin OpenRouter routing (`@auto` clears pin)."
            }
            "refresh-model-list" => {
                "`/refresh-model-list`\nForce-refresh provider model catalogs, update `/model`, and persist the refreshed cache."
            }
            "agents" => {
                "`/agents`\nOpen the agent-model config picker.\n\n`/agents <swarm|review|judge|memory|ambient>`\nJump straight to that agent role's saved model override."
            }
            "subagent" => {
                "`/subagent <prompt>`\nLaunch a subagent immediately.\n\nOptional flags:\n- `--type <kind>` sets the subagent type (default `general`)\n- `--model <name>` overrides the subagent model for this run\n- `--continue <session_id>` resumes an existing subagent session"
            }
            "observe" => {
                "`/observe`\nToggle transient observe mode for the side panel.\n\n`/observe on`\nEnable observe mode and focus the observe page.\n\n`/observe off`\nDisable observe mode.\n\n`/observe status`\nShow whether observe mode is enabled.\n\nObserve mode shows only the latest tool call or tool result added to context, and it is not persisted to disk."
            }
            "todos" => {
                "`/todos`\nToggle a transient todo screen in the side panel.\n\n`/todos on`\nEnable the dedicated todo screen and focus it.\n\n`/todos off`\nDisable the dedicated todo screen.\n\n`/todos status`\nShow whether the dedicated todo screen is enabled.\n\nThis view shows only the current session's todo list and refreshes as it changes."
            }
            "splitview" | "split-view" => {
                "`/splitview`\nToggle a transient split view that mirrors the current chat in the side panel.\n\n`/splitview on`\nEnable split view and focus the mirrored chat page.\n\n`/splitview off`\nDisable split view.\n\n`/splitview status`\nShow whether split view is enabled.\n\nThis gives the side panel its own scroll position for the same conversation so you can read older context while keeping the main composer active."
            }
            "btw" => {
                "`/btw <question>`\nAsk a side question about the current session and route the answer into the side panel.\n\nCurrent v1 behavior:\n- uses the side panel as the response surface\n- asks only from current session context\n- should not read files or run tools other than `side_panel`"
            }
            "git" => {
                "`/git`\nShow `git status --short --branch` for the current session working directory.\n\n`/git status`\nAlias for `/git`."
            }
            "catchup" => {
                "`/catchup`\nOpen the Catch Up picker for finished sessions that need attention.\n\n`/catchup next`\nTeleport to the next session needing attention and open a Catch Up brief in the side panel.\n\n`/catchup list`\nAlias for opening the picker."
            }
            "back" => {
                "`/back`\nReturn to the previous session you came from via Catch Up.\n\nWorks after a `/catchup next` jump or after selecting a session from the Catch Up picker."
            }
            "subagent-model" => {
                "`/subagent-model`\nShow the current subagent model policy for this session.\n\n`/subagent-model <name>`\nPin a fixed model for future subagents in this session.\n\n`/subagent-model inherit`\nReset to using the current active model."
            }
            "autoreview" => {
                "`/autoreview`\nShow autoreview status for this session.\n\n`/autoreview on`\nEnable end-of-turn autoreview for this session.\n\n`/autoreview off`\nDisable autoreview for this session.\n\n`/autoreview now`\nLaunch a headed reviewer immediately in a new window."
            }
            "autojudge" => {
                "`/autojudge`\nShow autojudge status for this session.\n\n`/autojudge on`\nEnable end-of-turn autojudge for this session. The autojudge acts like a completion manager: it tells the parent agent either to continue with specific next steps or that it is fine to stop.\n\n`/autojudge off`\nDisable autojudge for this session.\n\n`/autojudge now`\nLaunch a headed autojudge immediately in a new window."
            }
            "review" => {
                "`/review`\nLaunch a one-shot headed review session immediately.\n\nThe reviewer will DM this session when done. If OpenAI ChatGPT OAuth is available, it prefers `gpt-5.5`."
            }
            "judge" => {
                "`/judge`\nLaunch a one-shot headed judge session immediately.\n\nThe judge will DM this session when done. If OpenAI ChatGPT OAuth is available, it prefers `gpt-5.5`."
            }
            "effort" => {
                "`/effort`\nShow current reasoning effort.\n\n`/effort <level>`\nSet reasoning effort (none|low|medium|high|xhigh).\n\nAlso: Alt+←/→ to cycle."
            }
            "fast" => {
                "`/fast`\nShow whether OpenAI/Codex fast mode is enabled, plus the saved default.\n\n`/fast on`\nEnable fast mode (`service_tier = \"priority\"`) for the current session.\n\n`/fast off`\nDisable fast mode for the current session.\n\n`/fast status`\nShow current fast-mode status.\n\n`/fast default on`\nSave fast mode as the default on startup.\n\n`/fast default off`\nSave fast mode as the default off on startup.\n\n`/fast default status`\nShow the saved fast-mode default."
            }
            "memory" => "`/memory [on|off|status]`\nToggle memory features for this session.",
            "goals" => {
                "`/goals`\nOpen the goals overview in the side panel.\n\n`/goals resume`\nResume the most relevant active goal for this session/project.\n\n`/goals show <id>`\nOpen a specific goal in the side panel."
            }
            "swarm" => "`/swarm [on|off|status]`\nToggle swarm features for this session.",
            "dictate" | "dictation" => {
                "`/dictate`\nRun the configured external speech-to-text command and inject the transcript into jcode.\n\nConfigure `[dictation]` in `~/.jcode/config.toml`:\n- `command`: shell command that prints transcript to stdout, for example `~/.local/bin/my-whisper-script --grammar-target code`\n- `mode`: `insert|append|replace|send`\n- `key`: optional hotkey (for example `alt+;`)\n- `timeout_secs`: max wait time"
            }
            "poke" => {
                "`/poke [on|off|status]`\nPoke the model to resume when it has stopped with incomplete todos.\n\
                Auto-poke now starts enabled by default, and `Ctrl+P` toggles it on/off.\n\
                `/poke` or `/poke on` arms auto-poke and immediately pokes if work remains.\n\
                `/poke off` disarms auto-poke and clears any queued poke follow-ups.\n\
                `/poke status` shows whether auto-poke is currently armed.\n\
                If a turn is currently running, the poke is queued and sent right after that turn finishes.\n\
                Injects a reminder with the number of incomplete todos and prompts the model to either\n\
                finish the work, update the todo list to reflect what is done, or ask for user input if genuinely blocked."
            }
            "transfer" => {
                "`/transfer`\nCompact the current session into a summary-only handoff, copy the current todo list to a fresh session, and open that transferred session in a new window.\n\nIf a turn is currently running, jcode first soft-pauses the current session at the next safe point, then performs the transfer."
            }
            "improve" => {
                "`/improve [focus]`\nStart an autonomous repo-improvement loop. The model inspects the project, writes a ranked todo list, implements the highest-leverage safe improvements, validates them, then keeps going until further work has diminishing returns.\n\n`/improve plan [focus]`\nGenerate a ranked improve todo list only, without editing files.\n\n`/improve resume`\nResume the last saved improve mode for this session using the current improve todos.\n\n`/improve status`\nShow the inferred status of the current improve run and todo batch.\n\n`/improve stop`\nAsk the model to stop after the next safe point, update todos, and summarize remaining work."
            }
            "refactor" => {
                "`/refactor [focus]`\nStart a refactor loop aimed at moving the repo toward a practical 10/10. The main agent inspects the project, writes a ranked refactor todo list, implements the best safe refactors itself, validates each batch, and asks one independent read-only subagent to review each meaningful batch before continuing.\n\n`/refactor plan [focus]`\nGenerate a ranked refactor todo list only, without editing files.\n\n`/refactor resume`\nResume the last saved refactor mode for this session using the current refactor todos.\n\n`/refactor status`\nShow the inferred status of the current refactor run and todo batch.\n\n`/refactor stop`\nAsk the model to stop after the next safe point, update todos, and summarize remaining work."
            }
            "reload" => {
                "`/reload`\nReload into the newest available binary if one is ready. This is fast and does not rebuild."
            }
            "restart" => {
                "`/restart`\nRestart jcode with the current binary. Session is preserved.\nUseful after config changes, MCP server updates, or env var changes."
            }
            "rebuild" => {
                "`/rebuild`\nRun `git pull --ff-only`, `cargo build --release`, and release tests in the background. jcode stays usable and reloads automatically when the build is ready."
            }
            "selfdev" => {
                "`/selfdev`\nSpawn a new self-dev jcode session in a separate terminal.\n\n`/selfdev <prompt>`\nSpawn a new self-dev session and auto-deliver the prompt to it.\n\n`/selfdev status`\nShow current self-dev/build status."
            }
            "split" => {
                "`/split`\nSplit the current session into a new window. Clones the full conversation history so both sessions continue from the same point."
            }
            "resume" | "sessions" => {
                "`/resume`\nOpen the interactive session picker. Browse and search all sessions, preview conversation history, and open any session in a new terminal window.\n\nPress `Esc` to return to your current session."
            }
            "info" => "`/info`\nShow session metadata and token usage.",
            "context" => {
                "`/context`\nShow the full session context snapshot: prompt/context composition, compaction state, model/provider/runtime details, queued work, todos, and side-panel state."
            }
            "usage" => {
                "`/usage`\nFetch and display usage limits for connected providers. This command only reports real connected-provider usage windows and reset times."
            }
            "subscription" => {
                "`/subscription`\nShow curated jcode subscription status for this session, including router config, runtime mode, curated models, and planned tier budget scaffolding."
            }
            "version" => "`/version`\nShow jcode version/build details.",
            "changelog" => "`/changelog`\nShow recent changes embedded in this build.",
            "quit" => "`/quit`\nExit jcode.",
            "config" => {
                "`/config`\nShow active configuration.\n\n`/config init`\nCreate default config file.\n\n`/config edit`\nOpen config in `$EDITOR`."
            }
            "alignment" => {
                "`/alignment`\nShow the current alignment and the saved default.\n\n`/alignment centered`\nSave centered alignment as the default and apply it immediately.\n\n`/alignment left`\nSave left-aligned mode as the default and apply it immediately.\n\nPress `Alt+C` anytime to toggle alignment just for the current session."
            }
            "auth" | "login" => {
                "`/auth`\nShow authentication status for all providers.\n\n`/login`\nInteractive provider selection - pick a provider to log into.\n\n`/login <provider>`\nStart login flow directly for any provider shown by `/login` or the `/login ` completions.\n\nUse `/login jcode` for curated jcode subscription access via your router, not OpenRouter BYOK."
            }
            "account" | "accounts" => {
                "`/account`\nOpen the inline account picker showing both Claude and OpenAI accounts together. It lists saved accounts plus new/replace actions for each provider.\n\n`/account claude` / `/account openai`\nOpen the inline picker filtered to that provider.\n\n`/account <provider> settings`\nShow provider-specific account/settings details.\n\n`/account <provider> login`\nStart or refresh credentials for a provider.\n\n`/account claude add` / `/account openai add`\nCreate the next numbered OAuth account directly.\n\n`/account <provider> switch <label>`\nSwitch the active account for multi-account providers.\n\n`/account <provider> remove <label>`\nRemove a saved account.\n\n`/account default-provider <provider|auto>`\nSet the preferred default provider for future sessions.\n\n`/account default-model <model|clear>`\nSet the preferred default model for future sessions.\n\nOpenAI-specific settings: `/account openai transport ...`, `/account openai effort ...`, `/account openai fast on|off`.\n\nCustom provider settings: `/account openai-compatible api-base ...`, `api-key-name ...`, `env-file ...`, `default-model ...`."
            }
            "save" => {
                "`/save`\nBookmark the current session so it appears at the top of `/resume`.\n\n`/save <label>`\nBookmark with a custom label for easy identification.\n\nSaved sessions are shown in a dedicated \"Saved\" section in the session picker."
            }
            "unsave" => "`/unsave`\nRemove the bookmark from the current session.",
            "client-reload" if self.is_remote => {
                "`/client-reload`\nForce client binary reload in remote mode."
            }
            "server-reload" if self.is_remote => {
                "`/server-reload`\nForce server binary reload in remote mode."
            }
            _ => return None,
        };
        Some(help.to_string())
    }
}

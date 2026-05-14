# CarpAI Shell Completion Script
# Source this file or add to your shell config

# Bash completion
_carpai_completions() {
    local cur prev words cword
    _init_completion || return

    local commands="help build test commit push pull status diff log branch merge review plan task config clear export session compact rethink undo redo cost model plugin ssh"
    local subcommands_task="create list get update delete stats"
    local subcommands_plugin="list add remove enable disable install"
    local subcommands_ssh="connect exec upload download"

    case ${prev} in
        carpai)
            COMPREPLY=($(compgen -W "${commands}" -- "${cur}"))
            ;;
        task)
            COMPREPLY=($(compgen -W "${subcommands_task}" -- "${cur}"))
            ;;
        plugin)
            COMPREPLY=($(compgen -W "${subcommands_plugin}" -- "${cur}"))
            ;;
        ssh)
            COMPREPLY=($(compgen -W "${subcommands_ssh}" -- "${cur}"))
            ;;
        *)
            ;;
    esac
}

complete -F _carpai_completions carpai

# Aliases for common operations
alias cb='carpai build'
alias ct='carpai test'
alias cc='carpai commit'
alias cs='carpai status'
alias cr='carpai review'
alias cp='carpai plan'

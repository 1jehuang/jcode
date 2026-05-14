# CarpAI PowerShell Completion Module
# Import-Module ./carpai-completion.psm1

function ScriptBlock_Create {
    {
        param($commandName, $parameterName, $wordToComplete, $commandAst, $fakeBoundParameter)

        $commands = @(
            'help', 'build', 'test', 'commit', 'push', 'pull',
            'status', 'diff', 'log', 'branch', 'merge', 'review',
            'plan', 'task', 'config', 'clear', 'export', 'session',
            'compact', 'rethink', 'undo', 'redo', 'cost', 'model',
            'plugin', 'ssh'
        )

        $taskCommands = @('create', 'list', 'get', 'update', 'delete', 'stats')
        $pluginCommands = @('list', 'add', 'remove', 'enable', 'disable', 'install')
        $sshCommands = @('connect', 'exec', 'upload', 'download')

        if ($parameterName -eq 'Subcommand') {
            switch ($fakeBoundParameter.Command) {
                'task' { $taskCommands | Where-Object { $_ -like "$wordToComplete*" } }
                'plugin' { $pluginCommands | Where-Object { $_ -like "$wordToComplete*" } }
                'ssh' { $sshCommands | Where-Object { $_ -like "$wordToComplete*" } }
                Default { $commands | Where-Object { $_ -like "$wordToComplete*" } }
            }
        }
    }.GetNewClosure()
}

Register-ArgumentCompleter -CommandName 'carpai' -ParameterName 'Subcommand' -ScriptBlock (ScriptBlock_Create)

# Common aliases
Set-Alias -Name cb -Value 'carpai build' -Option Constant
Set-Alias -Name ct -Value 'carpai test' -Option Constant
Set-Alias -Name cc -Value 'carpai commit' -Option Constant
Set-Alias -Name cs -Value 'carpai status' -Option Constant

Export-ModuleMember -Function * -Alias *

# Thinking spacing

Thinking uses the original transcript spacing by default, with a blank row before a thinking block.
To opt into compact spacing, add this to `~/.jcode/config.toml` (or `$JCODE_HOME/config.toml`):

```toml
[display]
compact_thinking = true
```

Set `compact_thinking = false`, or omit it, to restore the original spacing.
This setting applies to retained reasoning blocks, reasoning embedded in assistant messages,
and live-streaming thinking. It does not change reasoning visibility or normal answer spacing.

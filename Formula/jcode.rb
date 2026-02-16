class Jcode < Formula
  desc "AI coding agent with TUI - uses Claude, OpenAI, or OpenRouter"
  homepage "https://github.com/jcode-cli/jcode"
  license "MIT"
  head "https://github.com/jcode-cli/jcode.git", branch: "main"

  # Uncomment and update for versioned releases:
  # url "https://github.com/jcode-cli/jcode/archive/refs/tags/v0.1.3.tar.gz"
  # sha256 "PLACEHOLDER"
  # version "0.1.3"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  def caveats
    <<~EOS
      jcode requires at least one AI provider configured:

      Claude (OAuth - recommended):
        Install Claude Code CLI: npm install -g @anthropic-ai/claude-code
        Then run: claude login

      OpenAI/Codex (OAuth):
        Run: codex login

      OpenRouter (API key):
        export OPENROUTER_API_KEY=sk-or-v1-...

      Direct Anthropic API key:
        export ANTHROPIC_API_KEY=sk-ant-...

      Data is stored in ~/.jcode/
    EOS
  end

  test do
    assert_match "jcode", shell_output("#{bin}/jcode --version")
  end
end

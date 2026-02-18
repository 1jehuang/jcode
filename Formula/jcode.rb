class Jcode < Formula
  desc "AI coding agent with TUI - uses Claude, OpenAI, or OpenRouter"
  homepage "https://github.com/1jehuang/jcode"
  license "MIT"
  head "https://github.com/1jehuang/jcode.git", branch: "master"

  url "https://github.com/1jehuang/jcode/archive/refs/tags/v0.3.0.tar.gz"
  version "0.3.0"

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

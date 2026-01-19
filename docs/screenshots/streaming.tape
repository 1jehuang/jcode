# VHS tape for capturing jcode streaming response
# Run with: vhs streaming.tape

Output streaming.png
Output streaming.gif

Set FontFamily "JetBrains Mono"
Set FontSize 14
Set Width 1200
Set Height 800
Set Theme "Catppuccin Mocha"
Set TypingSpeed 30ms
Set Padding 20

# Start jcode
Type "jcode"
Enter
Sleep 2s

# Send a message that will get a streaming response
Type "What is the capital of France?"
Enter

# Wait a moment to capture the streaming state
Sleep 2s

# Screenshot during streaming
Screenshot streaming.png

# Let it complete
Sleep 5s

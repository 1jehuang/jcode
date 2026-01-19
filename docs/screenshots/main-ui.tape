# VHS tape for capturing jcode UI screenshots
# Run with: vhs main-ui.tape

Output main-ui.png
Output main-ui.gif

Set FontFamily "JetBrains Mono"
Set FontSize 14
Set Width 1200
Set Height 800
Set Theme "Catppuccin Mocha"
Set TypingSpeed 50ms
Set Padding 20

# Start jcode
Type "jcode"
Enter
Sleep 2s

# Show the main UI with a message
Type "Hello! This is jcode - an AI coding assistant."
Sleep 500ms

# Take screenshot of main UI
Screenshot main-ui.png

Sleep 1s

# VHS tape for capturing jcode info widget
# Run with: vhs info-widget.tape

Output info-widget.png
Output info-widget.gif

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

# Toggle info widget with Ctrl+I
Ctrl+i
Sleep 500ms

# Take screenshot showing info widget
Screenshot info-widget.png

Sleep 500ms

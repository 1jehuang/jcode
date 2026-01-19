# VHS tape for capturing jcode session picker
# Run with: vhs session-picker.tape

Output session-picker.png
Output session-picker.gif

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

# Open session picker with /
Type "/"
Sleep 1s

# Take screenshot
Screenshot session-picker.png

Sleep 500ms

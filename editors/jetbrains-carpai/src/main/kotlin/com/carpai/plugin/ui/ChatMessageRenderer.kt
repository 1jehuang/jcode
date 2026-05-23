package com.carpai.plugin.ui

import com.intellij.ui.JBColor
import java.awt.Component
import javax.swing.DefaultListCellRenderer
import javax.swing.JList
import javax.swing.JPanel
import javax.swing.JTextArea

/**
 * Custom renderer for chat messages.
 */
class ChatMessageRenderer : DefaultListCellRenderer() {

    override fun getListCellRendererComponent(
        list: JList<*>,
        value: Any?,
        index: Int,
        isSelected: Boolean,
        cellHasFocus: Boolean
    ): Component {
        val panel = JPanel()
        val textArea = JTextArea()

        if (value is CarpaiChatPanel.ChatMessage) {
            val prefix = when (value.role) {
                CarpaiChatPanel.Role.USER -> "You: "
                CarpaiChatPanel.Role.ASSISTANT -> "CarpAI: "
                CarpaiChatPanel.Role.SYSTEM -> "System: "
            }

            textArea.text = prefix + value.content
            textArea.isEditable = false
            textArea.lineWrap = true
            textArea.wrapStyleWord = true

            // Different background colors for different roles
            val bgColor = when (value.role) {
                CarpaiChatPanel.Role.USER -> JBColor(0xF0F0F0, 0x3C3F41)
                CarpaiChatPanel.Role.ASSISTANT -> JBColor(0xE8F4F8, 0x2D2F31)
                CarpaiChatPanel.Role.SYSTEM -> JBColor(0xFFF3CD, 0x3D3420)
            }
            textArea.background = bgColor

            panel.add(textArea)
        }

        return panel
    }
}

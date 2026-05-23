package com.carpai.plugin.ui

import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.ToolWindowManager
import com.intellij.ui.components.JBScrollPane
import com.intellij.util.ui.JBUI
import mu.KotlinLogging
import java.awt.BorderLayout
import javax.swing.*

private val log = KotlinLogging.logger {}

/**
 * Main chat panel for CarpAI tool window.
 */
class CarpaiChatPanel(private val project: Project) : JPanel(BorderLayout()) {

    private val messageList = DefaultListModel<ChatMessage>()
    private val chatList = JList(messageList)
    private val inputField = JTextArea(3, 40)
    private val sendButton = JButton("Send")

    init {
        initializeUI()
        setupListeners()
    }

    private fun initializeUI() {
        border = JBUI.Borders.empty(10)

        // Chat messages area
        chatList.cellRenderer = ChatMessageRenderer()
        val scrollPane = JBScrollPane(chatList)
        scrollPane.verticalScrollBarPolicy = JScrollPane.VERTICAL_SCROLLBAR_AS_NEEDED
        add(scrollPane, BorderLayout.CENTER)

        // Input area
        val inputPanel = JPanel(BorderLayout())
        inputPanel.add(JScrollPane(inputField), BorderLayout.CENTER)
        inputPanel.add(sendButton, BorderLayout.EAST)
        add(inputPanel, BorderLayout.SOUTH)
    }

    private fun setupListeners() {
        sendButton.addActionListener {
            sendMessage()
        }

        inputField.addKeyListener(object : java.awt.event.KeyAdapter() {
            override fun keyPressed(e: java.awt.event.KeyEvent) {
                if (e.keyCode == java.awt.event.KeyEvent.VK_ENTER && !e.isShiftDown) {
                    e.consume()
                    sendMessage()
                }
            }
        })
    }

    private fun sendMessage() {
        val message = inputField.text.trim()
        if (message.isEmpty()) return

        // Add user message to chat
        messageList.addElement(ChatMessage(Role.USER, message))

        // Clear input
        inputField.text = ""

        // TODO: Send message to CarpAI server and get response
        // For now, add a placeholder response
        messageList.addElement(ChatMessage(Role.ASSISTANT, "Processing your request..."))

        log.debug { "Sent message: $message" }
    }

    /**
     * Show the CarpAI tool window.
     */
    fun show() {
        val toolWindow = ToolWindowManager.getInstance(project).getToolWindow("CarpAI")
        toolWindow?.show()
    }

    /**
     * Chat message data class.
     */
    data class ChatMessage(val role: Role, val content: String)

    enum class Role {
        USER, ASSISTANT, SYSTEM
    }
}

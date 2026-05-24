package com.carpai.ide.ui

import com.carpai.ide.client.GrpcClient
import com.carpai.ide.client.userMessage
import com.carpai.ide.config.CarpAiSettings
import com.intellij.openapi.project.Project
import com.intellij.ui.components.JBScrollPane
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.JBUI
import kotlinx.coroutines.*
import java.awt.BorderLayout
import javax.swing.*
import javax.swing.border.EmptyBorder

class ChatPanel(private val project: Project) : JPanel(BorderLayout()) {
    private val messagesModel = DefaultListModel<String>()
    private val messagesList = JList(messagesModel)
    private val inputField = JBTextField()
    private val sendButton = JButton("Send")

    private val settings = CarpAiSettings.getInstance(project)
    private lateinit var grpcClient: GrpcClient
    private val coroutineScope = CoroutineScope(Dispatchers.Main + SupervisorJob())

    init {
        initGrpcClient()
        setupUI()
        setupListeners()
    }

    private fun initGrpcClient() {
        grpcClient = GrpcClient(
            host = settings.serverHost,
            port = settings.serverPort,
            apiKey = settings.apiKey
        )
    }

    private fun setupUI() {
        border = EmptyBorder(8, 8, 8, 8)

        // Messages list
        messagesList.cellRenderer = MessageRenderer()
        val scrollPane = JBScrollPane(messagesList)
        scrollPane.verticalScrollBarPolicy = JScrollPane.VERTICAL_SCROLLBAR_AS_NEEDED
        add(scrollPane, BorderLayout.CENTER)

        // Input area
        val inputPanel = JPanel(BorderLayout())
        inputPanel.border = JBUI.Borders.emptyTop(8)
        inputPanel.add(inputField, BorderLayout.CENTER)
        inputPanel.add(sendButton, BorderLayout.EAST)
        add(inputPanel, BorderLayout.SOUTH)
    }

    private fun setupListeners() {
        sendButton.addActionListener { sendMessage() }
        inputField.addActionListener { sendMessage() }
    }

    private fun sendMessage() {
        val text = inputField.text.trim()
        if (text.isEmpty()) return

        messagesModel.addElement("You: $text")
        inputField.text = ""
        sendButton.isEnabled = false

        // Call gRPC server asynchronously
        coroutineScope.launch {
            try {
                val response = grpcClient.chatCompletion(
                    messages = listOf(userMessage(text)),
                    model = settings.defaultModel
                )

                response.onSuccess { completionResponse ->
                    SwingUtilities.invokeLater {
                        messagesModel.addElement("CarpAI: ${completionResponse.message.content}")
                        sendButton.isEnabled = true
                    }
                }.onFailure { error ->
                    SwingUtilities.invokeLater {
                        messagesModel.addElement("Error: ${error.message}")
                        sendButton.isEnabled = true
                    }
                }
            } catch (e: Exception) {
                SwingUtilities.invokeLater {
                    messagesModel.addElement("Error: ${e.message}")
                    sendButton.isEnabled = true
                }
            }
        }
    }

    fun dispose() {
        coroutineScope.cancel()
        grpcClient.close()
    }
}

// Simple message renderer
class MessageRenderer : ListCellRenderer<String> {
    override fun getListCellRendererComponent(
        list: JList<out String>,
        value: String,
        index: Int,
        isSelected: Boolean,
        cellHasFocus: Boolean
    ): Component {
        val label = JLabel(value)
        label.border = EmptyBorder(4, 8, 4, 8)
        if (value.startsWith("You:")) {
            label.background = UIManager.getColor("Button.background")
        } else {
            label.background = UIManager.getColor("Panel.background")
        }
        label.isOpaque = true
        return label
    }
}

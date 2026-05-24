package com.carpai.ide.config

import com.intellij.openapi.project.Project
import com.intellij.ui.components.JBTextField
import com.intellij.ui.components.JBLabel
import com.intellij.util.ui.FormBuilder
import com.intellij.util.ui.JBUI
import java.awt.BorderLayout
import javax.swing.JComponent
import javax.swing.JPanel
import javax.swing.JTextField

class CarpAiSettingsPanel(private val project: Project) {
    private val hostField = JBTextField()
    private val portField = JBTextField()
    private val apiKeyField = JBTextField()
    private val modelField = JBTextField()

    val panel: JPanel

    init {
        panel = FormBuilder.createFormBuilder()
            .addLabeledComponent(JBLabel("Server Host:"), hostField, 1, false)
            .addLabeledComponent(JBLabel("Server Port:"), portField, 1, false)
            .addLabeledComponent(JBLabel("API Key:"), apiKeyField, 1, false)
            .addLabeledComponent(JBLabel("Default Model:"), modelField, 1, false)
            .addComponentFillVertically(JPanel(), 0)
            .panel
    }

    var serverHost: String
        get() = hostField.text
        set(value) { hostField.text = value }

    var serverPort: Int
        get() = portField.text.toIntOrNull() ?: 50051
        set(value) { portField.text = value.toString() }

    var apiKey: String
        get() = apiKeyField.text
        set(value) { apiKeyField.text = value }

    var defaultModel: String
        get() = modelField.text
        set(value) { modelField.text = value }
}

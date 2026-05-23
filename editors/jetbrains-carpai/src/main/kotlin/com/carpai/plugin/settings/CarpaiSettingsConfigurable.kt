package com.carpai.plugin.settings

import com.intellij.openapi.options.Configurable
import com.intellij.openapi.ui.DialogPanel
import com.intellij.ui.components.JBTextField
import com.intellij.ui.dsl.builder.*
import javax.swing.JComponent

/**
 * Settings UI for CarpAI plugin.
 */
class CarpaiSettingsConfigurable : Configurable {

    private var settingsPanel: DialogPanel? = null
    private val settings = CarpaiSettings.getInstance()

    // UI components
    private val serverUrlField = JBTextField(settings.serverUrl)
    private val apiKeyField = JBTextField(settings.apiKey).apply {
        echoChar = '*'
    }
    private val defaultModelField = JBTextField(settings.defaultModel)

    override fun getDisplayName(): String {
        return "CarpAI Assistant"
    }

    override fun createComponent(): JComponent {
        settingsPanel = panel {
            group("Server Configuration") {
                row("Server URL:") {
                    cell(serverUrlField)
                        .comment("e.g., http://localhost:8081 or https://carpai.example.com")
                }
                row("API Key:") {
                    cell(apiKeyField)
                        .comment("Leave empty for anonymous access")
                }
                row {
                    checkBox("Enable SSL", settings.enableSsl)
                        .onApply { settings.enableSsl = it.isSelected }
                        .onReset { it.isSelected = settings.enableSsl }
                }
            }

            group("Model Configuration") {
                row("Default Model:") {
                    cell(defaultModelField)
                }
                row {
                    checkBox("Enable Auto Model Routing", settings.enableAutoModelRouting)
                        .onApply { settings.enableAutoModelRouting = it.isSelected }
                        .onReset { it.isSelected = settings.enableAutoModelRouting }
                }
            }

            group("Collaboration") {
                row {
                    checkBox("Enable Real-time Collaboration", settings.enableCollaboration)
                        .onApply { settings.enableCollaboration = it.isSelected }
                        .onReset { it.isSelected = settings.enableCollaboration }
                }
                row("Sync Interval (ms):") {
                    intTextField(100..30000, 100)
                        .bindText(settings.syncIntervalMs.toString())
                }
            }

            group("Advanced") {
                row("Request Timeout (ms):") {
                    intTextField(5000..120000, 1000)
                        .bindText(settings.requestTimeoutMs.toString())
                }
                row("Max Context Tokens:") {
                    intTextField(1024..32768, 1024)
                        .bindText(settings.maxContextTokens.toString())
                }
            }
        }

        return settingsPanel!!
    }

    override fun isModified(): Boolean {
        return serverUrlField.text != settings.serverUrl ||
               apiKeyField.text != settings.apiKey ||
               defaultModelField.text != settings.defaultModel
    }

    override fun apply() {
        settings.serverUrl = serverUrlField.text
        settings.apiKey = apiKeyField.text
        settings.defaultModel = defaultModelField.text
    }

    override fun reset() {
        serverUrlField.text = settings.serverUrl
        apiKeyField.text = settings.apiKey
        defaultModelField.text = settings.defaultModel
    }

    override fun disposeUIResources() {
        settingsPanel = null
    }
}

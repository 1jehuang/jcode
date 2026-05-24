package com.carpai.ide.config

import com.intellij.openapi.options.Configurable
import com.intellij.openapi.project.Project
import javax.swing.JComponent

class CarpAiSettingsConfigurable(private val project: Project) : Configurable {
    private var settingsPanel: CarpAiSettingsPanel? = null

    override fun getDisplayName(): String = "CarpAI"

    override fun createComponent(): JComponent {
        settingsPanel = CarpAiSettingsPanel(project)
        return settingsPanel!!.panel
    }

    override fun isModified(): Boolean {
        val settings = CarpAiSettings.getInstance(project)
        return settingsPanel?.let {
            it.serverHost != settings.serverHost ||
            it.serverPort != settings.serverPort ||
            it.apiKey != settings.apiKey ||
            it.defaultModel != settings.defaultModel
        } ?: false
    }

    override fun apply() {
        val settings = CarpAiSettings.getInstance(project)
        settingsPanel?.let {
            settings.serverHost = it.serverHost
            settings.serverPort = it.serverPort
            settings.apiKey = it.apiKey
            settings.defaultModel = it.defaultModel
        }
    }

    override fun reset() {
        val settings = CarpAiSettings.getInstance(project)
        settingsPanel?.let {
            it.serverHost = settings.serverHost
            it.serverPort = settings.serverPort
            it.apiKey = settings.apiKey
            it.defaultModel = settings.defaultModel
        }
    }

    override fun disposeUIResources() {
        settingsPanel = null
    }
}

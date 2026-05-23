package com.carpai.plugin.settings

import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.util.xmlb.XmlSerializerUtil

/**
 * Persistent settings for CarpAI plugin.
 */
@State(
    name = "CarpaiSettings",
    storages = [Storage("carpai-settings.xml")]
)
class CarpaiSettings : PersistentStateComponent<CarpaiSettings> {

    // Server configuration
    var serverUrl: String = "http://localhost:8081"
    var apiKey: String = ""
    var enableSsl: Boolean = false

    // Model configuration
    var defaultModel: String = "carpai-coder-v1"
    var enableAutoModelRouting: Boolean = true

    // Collaboration settings
    var enableCollaboration: Boolean = true
    var syncIntervalMs: Int = 5000

    // UI settings
    var showInlineCompletions: Boolean = true
    var enableChatNotifications: Boolean = true

    // Advanced settings
    var requestTimeoutMs: Int = 30000
    var maxContextTokens: Int = 8192

    override fun getState(): CarpaiSettings {
        return this
    }

    override fun loadState(state: CarpaiSettings) {
        XmlSerializerUtil.copyBean(state, this)
    }

    companion object {
        fun getInstance(): CarpaiSettings {
            return com.intellij.openapi.application.ApplicationManager.getApplication()
                .getService(CarpaiSettings::class.java)
        }
    }
}

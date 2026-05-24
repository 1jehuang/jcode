package com.carpai.ide.config

import com.intellij.openapi.components.*
import com.intellij.openapi.project.Project
import com.intellij.util.xmlb.XmlSerializerUtil

@Service(Service.Level.PROJECT)
@State(
    name = "CarpAiSettings",
    storages = [Storage("carpai-settings.xml")]
)
class CarpAiSettings : PersistentStateComponent<CarpAiSettings> {
    var serverHost: String = "localhost"
    var serverPort: Int = 50051
    var apiKey: String = ""
    var defaultModel: String = "claude-sonnet-4"

    companion object {
        fun getInstance(project: Project): CarpAiSettings {
            return project.service<CarpAiSettings>()
        }
    }

    override fun getState(): CarpAiSettings = this

    override fun loadState(state: CarpAiSettings) {
        XmlSerializerUtil.copyBean(state, this)
    }
}

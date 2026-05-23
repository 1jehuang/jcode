package com.carpai.plugin.ui

import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.ToolWindow
import com.intellij.openapi.wm.ToolWindowFactory
import com.intellij.ui.content.ContentFactory
import mu.KotlinLogging

private val log = KotlinLogging.logger {}

/**
 * Factory for creating the CarpAI tool window.
 */
class CarpaiToolWindowFactory : ToolWindowFactory {

    override fun createToolWindowContent(project: Project, toolWindow: ToolWindow) {
        log.info { "Creating CarpAI tool window" }

        val chatPanel = CarpaiChatPanel(project)
        val content = ContentFactory.getInstance().createContent(chatPanel, "", false)
        toolWindow.contentManager.addContent(content)

        log.info { "CarpAI tool window created" }
    }
}

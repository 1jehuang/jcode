package com.carpai.plugin.actions

import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.wm.ToolWindowManager
import mu.KotlinLogging

private val log = KotlinLogging.logger {}

/**
 * Action to open CarpAI chat panel.
 */
class OpenChatAction : AnAction() {

    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return

        log.info { "Opening CarpAI chat" }

        val toolWindow = ToolWindowManager.getInstance(project).getToolWindow("CarpAI")
        toolWindow?.show()
    }

    override fun update(e: AnActionEvent) {
        e.presentation.isEnabledAndVisible = e.project != null
    }
}

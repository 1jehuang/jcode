package com.carpai.plugin.actions

import com.carpai.plugin.CarpaiService
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import mu.KotlinLogging
import kotlinx.coroutines.runBlocking

private val log = KotlinLogging.logger {}

/**
 * Action to explain selected code using CarpAI server.
 * Sends code via HTTP to carpai-server API.
 */
class ExplainCodeAction : AnAction() {

    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        val editor = e.getData(CommonDataKeys.EDITOR) ?: return
        val selectedText = editor.selectionModel.selectedText

        if (selectedText.isNullOrEmpty()) {
            log.warn { "No code selected for explanation" }
            return
        }

        log.info { "Explaining code: ${selectedText.take(50)}..." }

        // Real API call via CarpaiService
        val service = project.getService(CarpaiService::class.java)
        val apiKey = com.carpai.plugin.settings.CarpaiSettings.getInstance().apiKey
        val serverUrl = com.carpai.plugin.settings.CarpaiSettings.getInstance().serverUrl

        try {
            val response = runBlocking {
                service.callApi("$serverUrl/api/v1/explain", apiKey, mapOf("code" to selectedText))
            }
            val explanation = response?.getString("explanation") ?: "No explanation returned."

            NotificationGroupManager.getInstance()
                .getNotificationGroup("carpai.notifications")
                .createNotification(
                    "Code Explanation",
                    explanation.take(300),
                    NotificationType.INFORMATION
                )
                .notify(project)
        } catch (ex: Exception) {
            log.error { "Failed to explain code: ${ex.message}" }
            NotificationGroupManager.getInstance()
                .getNotificationGroup("carpai.notifications")
                .createNotification(
                    "Code Explanation Failed",
                    "Could not connect to CarpAI server at $serverUrl",
                    NotificationType.ERROR
                )
                .notify(project)
        }
    }

    override fun update(e: AnActionEvent) {
        val editor = e.getData(CommonDataKeys.EDITOR)
        val hasSelection = editor?.selectionModel?.hasSelection() == true
        e.presentation.isEnabledAndVisible = hasSelection
    }
}

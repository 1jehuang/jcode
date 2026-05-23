package com.carpai.plugin.actions

import com.carpai.plugin.CarpaiService
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.ui.Messages
import com.intellij.notification.NotificationGroupManager
import com.intellij.notification.NotificationType
import mu.KotlinLogging
import kotlinx.coroutines.runBlocking

private val log = KotlinLogging.logger {}

/**
 * Action to refactor selected code using CarpAI server.
 */
class RefactorCodeAction : AnAction() {

    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        val editor = e.getData(CommonDataKeys.EDITOR) ?: return
        val selectedText = editor.selectionModel.selectedText

        if (selectedText.isNullOrEmpty()) {
            log.warn { "No code selected for refactoring" }
            return
        }

        // Ask user for refactoring instructions
        val instructions = Messages.showInputDialog(
            project,
            "Enter refactoring instructions:",
            "CarpAI Refactor",
            null,
            "e.g., extract to function, rename to camelCase",
        ) ?: return

        log.info { "Refactoring code: ${selectedText.take(50)}... with: $instructions" }

        val service = project.getService(CarpaiService::class.java)
        val apiKey = com.carpai.plugin.settings.CarpaiSettings.getInstance().apiKey
        val serverUrl = com.carpai.plugin.settings.CarpaiSettings.getInstance().serverUrl

        try {
            val response = runBlocking {
                service.callApi("$serverUrl/api/v1/refactor", apiKey, mapOf(
                    "code" to selectedText,
                    "instructions" to instructions
                ))
            }
            val refactored = response?.getString("refactored") ?: ""

            if (refactored.isNotEmpty()) {
                // Apply the refactored code
                val document = editor.document
                runBlocking {
                    com.intellij.openapi.command.WriteCommandAction.runWriteCommandAction(project) {
                        document.replaceString(
                            editor.selectionModel.selectionStart,
                            editor.selectionModel.selectionEnd,
                            refactored
                        )
                    }
                }
                NotificationGroupManager.getInstance()
                    .getNotificationGroup("carpai.notifications")
                    .createNotification("Code Refactored", "Applied ${refactored.lines().size} lines", NotificationType.INFORMATION)
                    .notify(project)
            }
        } catch (ex: Exception) {
            log.error { "Failed to refactor code: ${ex.message}" }
            NotificationGroupManager.getInstance()
                .getNotificationGroup("carpai.notifications")
                .createNotification("Refactoring Failed", ex.message ?: "Unknown error", NotificationType.ERROR)
                .notify(project)
        }
    }

    override fun update(e: AnActionEvent) {
        val editor = e.getData(CommonDataKeys.EDITOR)
        val hasSelection = editor?.selectionModel?.hasSelection() == true
        e.presentation.isEnabledAndVisible = hasSelection
    }
}

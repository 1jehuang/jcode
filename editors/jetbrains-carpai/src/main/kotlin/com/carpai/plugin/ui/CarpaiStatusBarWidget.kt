package com.carpai.plugin.ui

import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.StatusBarWidget
import com.intellij.openapi.wm.impl.status.EditorBasedStatusBarPopup
import com.intellij.util.ui.JBUI

/**
 * Status bar widget showing CarpAI connection status.
 */
class CarpaiStatusBarWidget(project: Project) : EditorBasedStatusBarPopup(project, false) {

    companion object {
        const val ID = "Carpai.StatusWidget"
    }

    override fun ID(): String = ID

    override fun createInstance(project: Project): StatusBarWidget {
        return CarpaiStatusBarWidget(project)
    }

    override fun getWidgetState(): WidgetState {
        // TODO: Check actual connection status
        val isConnected = true
        val text = if (isConnected) "CarpAI: Connected" else "CarpAI: Disconnected"
        val tooltip = "CarpAI AI Assistant"
        val icon = null // TODO: Add status icon

        return WidgetState(text, tooltip, true).apply {
            this.icon = icon
        }
    }
}

/**
 * Factory for creating the status bar widget.
 */
class CarpaiStatusBarWidgetFactory : StatusBarWidget.Factory {
    override fun getId(): String = CarpaiStatusBarWidget.ID

    override fun getDisplayName(): String = "CarpAI Status"

    override fun createWidget(project: com.intellij.openapi.project.Project): StatusBarWidget {
        return CarpaiStatusBarWidget(project)
    }
}

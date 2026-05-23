package com.carpai.plugin

import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity

/**
 * Main plugin initialization activity.
 * Runs when a project is opened.
 */
class CarpaiStartupActivity : ProjectActivity {
    override suspend fun execute(project: Project) {
        // Initialize CarpAI services
        CarpaiService.getInstance(project).initialize()
    }
}

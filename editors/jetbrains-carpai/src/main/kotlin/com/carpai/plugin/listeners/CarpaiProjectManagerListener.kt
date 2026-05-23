package com.carpai.plugin.listeners

import com.intellij.openapi.project.Project
import com.intellij.openapi.project.ProjectManagerListener
import com.carpai.plugin.CarpaiService
import mu.KotlinLogging

private val log = KotlinLogging.logger {}

/**
 * Listener for project lifecycle events.
 * Handles cleanup when projects are closed.
 */
class CarpaiProjectManagerListener : ProjectManagerListener {

    override fun projectClosed(project: Project) {
        log.info { "Project closed: ${project.name}" }

        // Dispose CarpAI services
        val service = project.getService(CarpaiService::class.java)
        service?.dispose()
    }

    override fun projectOpened(project: Project) {
        log.info { "Project opened: ${project.name}" }
        // Services are initialized by CarpaiStartupActivity
    }
}

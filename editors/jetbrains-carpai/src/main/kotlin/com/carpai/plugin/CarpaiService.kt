package com.carpai.plugin

import com.intellij.openapi.components.Service
import com.intellij.openapi.project.Project
import com.carpai.plugin.lsp.CarpaiLspClient
import com.carpai.plugin.collab.CollaborationService
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import mu.KotlinLogging

private val log = KotlinLogging.logger {}

/**
 * Main service for CarpAI plugin.
 * Manages lifecycle of LSP client, collaboration service, and other components.
 */
@Service(Service.Level.PROJECT)
class CarpaiService(private val project: Project, private val cs: CoroutineScope) {

    private val job = SupervisorJob()
    private val scope = CoroutineScope(Dispatchers.IO + job)

    private var lspClient: CarpaiLspClient? = null
    private var collaborationService: CollaborationService? = null

    companion object {
        fun getInstance(project: Project): CarpaiService {
            return project.getService(CarpaiService::class.java)
        }
    }

    /**
     * Initialize all CarpAI services.
     */
    fun initialize() {
        log.info { "Initializing CarpAI services for project: ${project.name}" }

        // Initialize LSP client
        lspClient = CarpaiLspClient(project)
        lspClient?.start()

        // Initialize collaboration service
        collaborationService = CollaborationService(project)

        log.info { "CarpAI services initialized successfully" }
    }

    /**
     * Get the LSP client instance.
     */
    fun getLspClient(): CarpaiLspClient? {
        return lspClient
    }

    /**
     * Get the collaboration service instance.
     */
    fun getCollaborationService(): CollaborationService? {
        return collaborationService
    }

    /**
     * Dispose resources when project is closed.
     */
    fun dispose() {
        log.info { "Disposing CarpAI services" }

        lspClient?.stop()
        collaborationService?.disconnect()

        job.cancel()
    }
}

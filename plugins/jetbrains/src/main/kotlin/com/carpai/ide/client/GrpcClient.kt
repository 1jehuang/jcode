package com.carpai.ide.client

import carpai.agent.*
import io.grpc.ManagedChannelBuilder
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.util.concurrent.TimeUnit

/**
 * gRPC client wrapper for CarpAI Server communication
 */
class GrpcClient(
    private val host: String = "localhost",
    private val port: Int = 50051,
    private val apiKey: String = ""
) {
    private val channel = ManagedChannelBuilder
        .forAddress(host, port)
        .usePlaintext() // Use TLS in production
        .build()

    private val agentStub = AgentServiceGrpc.newBlockingStub(channel)

    /**
     * Send a chat completion request
     */
    suspend fun chatCompletion(
        messages: List<ChatMessage>,
        model: String = "claude-sonnet-4"
    ): Result<ChatCompletionResponse> = withContext(Dispatchers.IO) {
        try {
            val request = ChatCompletionRequest.newBuilder()
                .addAllMessages(messages)
                .setModel(model)
                .setStream(false)
                .build()

            val response = agentStub.chatCompletion(request)
            Result.success(response)
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    /**
     * Create a new session
     */
    suspend fun createSession(
        title: String? = null,
        workingDir: String? = null,
        model: String? = null
    ): Result<SessionResponse> = withContext(Dispatchers.IO) {
        try {
            val builder = CreateSessionRequest.newBuilder()
            title?.let { builder.setTitle(it) }
            workingDir?.let { builder.setWorkingDir(it) }
            model?.let { builder.setModel(it) }

            val response = agentStub.createSession(builder.build())
            Result.success(response)
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    /**
     * Append a message to a session
     */
    suspend fun appendMessage(
        sessionId: String,
        role: String,
        content: String
    ): Result<Boolean> = withContext(Dispatchers.IO) {
        try {
            val message = ChatMessage.newBuilder()
                .setRole(role)
                .setContent(content)
                .build()

            val request = AppendMessageRequest.newBuilder()
                .setSessionId(sessionId)
                .setMessage(message)
                .build()

            val response = agentStub.appendMessage(request)
            Result.success(response.success)
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    /**
     * Get messages from a session
     */
    suspend fun getMessages(
        sessionId: String,
        limit: Int = 50
    ): Result<GetMessagesResponse> = withContext(Dispatchers.IO) {
        try {
            val request = GetMessagesRequest.newBuilder()
                .setSessionId(sessionId)
                .setLimit(limit)
                .build()

            val response = agentStub.getMessages(request)
            Result.success(response)
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    /**
     * Close the gRPC channel
     */
    fun close() {
        channel.shutdown().awaitTermination(5, TimeUnit.SECONDS)
    }
}

// Helper extension functions
fun userMessage(content: String): ChatMessage =
    ChatMessage.newBuilder().setRole("user").setContent(content).build()

fun assistantMessage(content: String): ChatMessage =
    ChatMessage.newBuilder().setRole("assistant").setContent(content).build()


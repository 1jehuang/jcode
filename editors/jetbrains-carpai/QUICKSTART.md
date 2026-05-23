# JetBrains Plugin Development Quick Start

## Project Scaffold Created ✅

The complete JetBrains plugin scaffold has been created at:
`editors/jetbrains-carpai/`

## File Structure Summary

### Build Configuration (3 files)
- `build.gradle.kts` - Gradle build with IntelliJ plugin support
- `settings.gradle.kts` - Project settings
- `gradle.properties` - Gradle and plugin properties

### Plugin Configuration (1 file)
- `src/main/resources/META-INF/plugin.xml` - Plugin manifest with actions, UI, services

### Kotlin Source Files (14 files)

**Core Services:**
- `CarpaiPlugin.kt` - Startup activity
- `CarpaiService.kt` - Main service manager (LSP + Collaboration)

**Settings:**
- `CarpaiSettings.kt` - Persistent settings storage
- `CarpaiSettingsConfigurable.kt` - Settings UI panel

**LSP Integration:**
- `CarpaiLspClient.kt` - Language Server Protocol client

**Collaboration:**
- `CollaborationService.kt` - Real-time collaboration with CRDT

**UI Components:**
- `CarpaiToolWindowFactory.kt` - Tool window factory
- `CarpaiChatPanel.kt` - Main chat interface
- `ChatMessageRenderer.kt` - Custom message rendering
- `CarpaiStatusBarWidget.kt` - Status bar indicator

**Actions:**
- `OpenChatAction.kt` - Open chat panel (Ctrl+Alt+C)
- `ExplainCodeAction.kt` - Explain selected code (Ctrl+Alt+E)
- `RefactorCodeAction.kt` - Refactor selected code (Ctrl+Alt+R)

**Listeners:**
- `CarpaiProjectManagerListener.kt` - Project lifecycle events

## Next Steps for Development

### Week 1: Setup & Basic Connectivity

1. **Install Prerequisites**
   ```bash
   # Install JDK 17
   sdk install java 17-amzn

   # Verify Gradle
   cd editors/jetbrains-carpai
   ./gradlew --version
   ```

2. **Run in Development Mode**
   ```bash
   ./gradlew runIde
   ```
   This will launch a sandbox IntelliJ instance with the plugin loaded.

3. **Implement HTTP Client**
   ```kotlin
   // In CarpaiLspClient.kt
   import io.ktor.client.*
   import io.ktor.client.request.*
   import io.ktor.client.statement.*

   val client = HttpClient(CIO) {
       install(ContentNegotiation) {
           json()
       }
   }

   suspend fun sendMessage(message: String): String {
       val response = client.post("${settings.serverUrl}/api/chat") {
           contentType(ContentType.Application.Json)
           setBody(ChatRequest(message))
       }
       return response.body<ChatResponse>().text
   }
   ```

### Week 2: Chat Functionality

4. **Connect Chat Panel to Server**
   ```kotlin
   // In CarpaiChatPanel.kt
   private fun sendMessage() {
       val message = inputField.text.trim()
       messageList.addElement(ChatMessage(Role.USER, message))

       // Async request to server
       scope.launch {
           val response = httpClient.postChat(message)
           withContext(Dispatchers.Main) {
               messageList.addElement(ChatMessage(Role.ASSISTANT, response))
           }
       }

       inputField.text = ""
   }
   ```

5. **Add Streaming Support**
   ```kotlin
   // For streaming responses
   client.ws(host = "localhost", port = 8081, path = "/ws/chat") {
       incoming.consumeAsFlow().collect { frame ->
           if (frame is Frame.Text) {
               val chunk = frame.readText()
               // Append to current message
           }
       }
   }
   ```

### Week 3: LSP Integration

6. **Implement Code Completion**
   ```kotlin
   // In CarpaiLspClient.kt
   fun requestCompletion(file: VirtualFile, line: Int, column: Int) {
       val params = CompletionParams(
           TextDocumentIdentifier(file.url),
           Position(line, column)
       )

       server?.textDocumentService?.completion(params)?.thenApply { list ->
           // Process completions
       }
   }
   ```

7. **Add Diagnostics**
   ```kotlin
   // Listen for diagnostics from server
   connection.onNotification(PublishDiagnosticsParams.METHOD) { params: PublishDiagnosticsParams ->
       // Show warnings/errors in editor
   }
   ```

### Week 4: Collaboration Features

8. **Integrate Yrs CRDT**
   ```kotlin
   // Add to build.gradle.kts dependencies
   implementation("com.github.yjs:yrs:0.17.0")

   // In CollaborationService.kt
   import yrs.*

   val doc = Doc()
   val text = doc.getText("code")

   // Observe remote changes
   text.observe { event ->
       // Apply changes to editor
   }

   // Send local changes
   text.insert(transaction, index, content)
   ```

9. **Cursor Synchronization**
   ```kotlin
   // Broadcast cursor position
   fun updateCursorPosition(line: Int, column: Int) {
       val cursorMap = doc.getMap("cursors")
       cursorMap.put(userId, mapOf(
           "line" to line,
           "column" to column,
           "timestamp" to System.currentTimeMillis()
       ))
   }
   ```

### Week 5-6: Polish & Testing

10. **Add Error Handling**
    ```kotlin
    try {
        val response = client.post(...)
    } catch (e: ConnectException) {
        showNotification("Cannot connect to CarpAI server", ERROR)
    } catch (e: TimeoutException) {
        showNotification("Request timed out", WARNING)
    }
    ```

11. **Write Tests**
    ```kotlin
    // src/test/kotlin/CarpaiSettingsTest.kt
    @Test
    fun testSettingsPersistence() {
        val settings = CarpaiSettings()
        settings.serverUrl = "http://test.example.com"

        val state = settings.state
        assertEquals("http://test.example.com", state.serverUrl)
    }
    ```

12. **Build Release**
    ```bash
    ./gradlew buildPlugin
    # Output: build/distributions/carpai-jetbrains-plugin-1.0.0.zip
    ```

## Testing Checklist

- [ ] Plugin loads without errors
- [ ] Tool window opens correctly
- [ ] Chat messages send/receive
- [ ] Settings persist across restarts
- [ ] Actions trigger from menu/shortcuts
- [ ] Status bar widget shows connection status
- [ ] No memory leaks on project close

## Publishing to Marketplace

1. **Create JetBrains Account**
   - Visit https://plugins.jetbrains.com
   - Register as a vendor

2. **Generate Publishing Token**
   - Go to Profile → Publishing Token
   - Create token with upload permissions

3. **Configure CI/CD**
   ```yaml
   # .github/workflows/publish.yml
   name: Publish Plugin
   on:
     release:
       types: [published]

   jobs:
     publish:
       runs-on: ubuntu-latest
       steps:
         - uses: actions/checkout@v3
         - name: Publish
           run: ./gradlew publishPlugin
           env:
             PUBLISH_TOKEN: ${{ secrets.JB_MARKETPLACE_TOKEN }}
   ```

4. **Submit for Review**
   ```bash
   ./gradlew publishPlugin
   ```

## Resources

- **IntelliJ Platform SDK**: https://plugins.jetbrains.com/docs/intellij/
- **Kotlin UI DSL**: https://plugins.jetbrains.com/docs/intellij/kotlin-ui-dsl.html
- **LSP4J Documentation**: https://github.com/eclipse/lsp4j
- **Yrs CRDT**: https://github.com/y-crdt/y-crdt

## Troubleshooting

### Plugin doesn't load
- Check `idea.log` for errors: Help → Show Log in Explorer
- Verify plugin.xml syntax
- Ensure all required dependencies are declared

### Gradle build fails
- Clear cache: `./gradlew clean`
- Update Gradle wrapper: `./gradlew wrapper --gradle-version 8.5`
- Check JDK version: `java -version` (must be 17+)

### LSP connection issues
- Verify server is running: `curl http://localhost:8081/health`
- Check firewall settings
- Enable debug logging in settings

---

**Status**: ✅ Scaffold Complete
**Next**: Implement HTTP client and chat functionality (Week 1)

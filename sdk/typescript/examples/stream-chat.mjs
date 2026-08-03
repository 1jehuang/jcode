/**
 * Streaming chat example.
 *
 * Requires a running bridge:
 *   jcode api-bridge
 *
 * Usage: node examples/stream-chat.mjs "your prompt"
 */

import { JcodeClient } from "../dist/index.js";

const prompt = process.argv.slice(2).join(" ") || "Say hello in five words.";

const client = await JcodeClient.connect({ clientName: "stream-chat-example/0.1" });
console.log(`connected to ${client.server} (${client.capabilities.join(", ")})`);

client.on("harness_error", (frame) => console.error("harness error:", frame.message));

const session = await client.createSession(process.cwd());
console.log(`session ${session.session_id}\n`);

await client.sendMessage(session.session_id, prompt);

for await (const event of client.events(session.session_id)) {
  switch (event.ev) {
    case "model_info":
      console.log(`[model] ${event.provider}/${event.model}\n`);
      break;
    case "text_delta":
      process.stdout.write(event.text);
      break;
    case "tool_start":
      console.log(`\n[tool] ${event.name}`);
      break;
    case "permission_request":
      console.log(`\n[permission] ${event.tool_name}: ${event.description} -> allow`);
      await client.respondToPermission(session.session_id, event.request_id, "allow");
      break;
    case "background_progress":
      console.log(`\n[bg ${event.task_id}] ${event.summary}`);
      break;
    case "token_usage":
      console.log(`\n\n[tokens] in=${event.input} out=${event.output}`);
      break;
    case "turn_done":
      client.close();
      process.exit(0);
  }
}

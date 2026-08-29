// Reasoning frames render the Thinking block at bundle level: a reply that
// streams scratch work before its content must leave a durable, expandable
// Thinking toggle in the feed, auto-collapsed once the answer streamed,
// revealing the preserved reasoning when expanded and rolling back up when
// collapsed. (The plugin's unit-level states live in
// test/thinking-block.mjs; this test pins the wire-to-feed path.)
// Run: node test/reasoning-thinking-block.mjs (after `npm run build`).
import { bootWorkbench } from "./helpers/boot.mjs";

await bootWorkbench("reasoning renders the Thinking block", async ({ chatSockets, history, submitChat, failures }) => {
  const socket = chatSockets[0];
  socket.reasonChat = true;
  await submitChat("Why?");
  socket.reasonChat = false;

  const thinkToggle = history.querySelector(".mur-think-toggle");
  const thinkContent = history.querySelector(".mur-think-content");
  if (!thinkToggle || !thinkContent) {
    failures.push("reasoning frames did not render a Thinking block in the feed");
    return;
  }
  if (!thinkToggle.textContent.includes("Thinking")) {
    failures.push(`the Thinking toggle label is "${thinkToggle.textContent}"`);
  }
  if (!thinkContent.hidden) {
    failures.push("the Thinking block did not auto-collapse after the answer streamed");
  }
  thinkToggle.click();
  if (thinkContent.hidden || !thinkContent.textContent.includes("consider the ask then answer")) {
    failures.push("expanding the completed Thinking block did not reveal the preserved reasoning");
  }
  thinkToggle.click();
  if (!thinkContent.hidden) {
    failures.push("the completed Thinking block did not roll back up");
  }
});

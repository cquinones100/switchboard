// Prints the latest reply of the Claude session in a tmux pane, read from its
// transcript, for M-y (tmux/reply-copy.sh), which copies it.
// Usage: node lib/claude-reply.js <pane id>
// Exits 3 when no session names the pane and 4 when it has no reply yet.

const { sessionsByPane, transcriptFor, lastReply } = require("./sidebar/claude");

const session = sessionsByPane().get(process.argv[2]);
if (!session) process.exit(3);
const file = transcriptFor(session);
const reply = file && lastReply(file);
if (!reply) process.exit(4);
process.stdout.write(reply);

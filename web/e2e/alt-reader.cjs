// A full-screen program that asked for the mouse, for tests/alt.e2e.ts:
// `node alt-reader.cjs <log>` writes to <log> every byte it is sent.
const fs = require("node:fs");
const log = process.argv[2];
fs.writeFileSync(log, "");
process.stdout.write("\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[Hfull screen reader\r\n");
process.stdin.setRawMode(true);
process.stdin.on("data", (d) => fs.appendFileSync(log, d.toString("latin1")));
setInterval(() => {}, 1000);

// dist/index.html.gz beside dist/index.html: what keepane sends a browser
// that takes gzip (a fifth of the bytes, which counts over Tailscale). The
// same input gives the same bytes (no file name or time in the header), so
// CI can check that what is committed is what the source builds.
import { readFileSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const html = readFileSync(new URL("../dist/index.html", import.meta.url));
const gz = gzipSync(html, { level: 9 });
// The header's time and OS bytes: zero / "unknown", whatever zlib wrote.
gz.writeUInt32LE(0, 4);
gz[9] = 255;
writeFileSync(new URL("../dist/index.html.gz", import.meta.url), gz);
console.log(`dist/index.html.gz ${gz.length} bytes (from ${html.length})`);

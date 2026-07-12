import { readFile, writeFile } from 'node:fs/promises';

const target = process.argv[2];
if (!target) {
  throw new Error('usage: patch-mcp-conformance-hosts.mjs <path>');
}

const old = 'const app = createMcpExpressApp();';
const replacement = "const app = createMcpExpressApp({ allowedHosts: ['mcp_conformance_server', 'localhost', '127.0.0.1', '::1'] });";
const source = await readFile(target, 'utf8');
const replacementCount = source.split(old).length - 1;

if (replacementCount !== 1) {
  throw new Error(`expected exactly one host patch target, found ${replacementCount}`);
}

await writeFile(target, source.replace(old, replacement));

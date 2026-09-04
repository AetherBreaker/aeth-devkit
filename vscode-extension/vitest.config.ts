import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// The pure modules never touch `vscode`, but the modules that import it must still load.
export default defineConfig({
  test: { include: ['test/**/*.test.ts'] },
  resolve: { alias: { vscode: fileURLToPath(new URL('./test/vscode-stub.ts', import.meta.url)) } },
});

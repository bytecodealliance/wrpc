// Serves the `echo-server` world of `tests/js/wit` over the codec tree on the
// address given as the first argument, for the Rust client in `tests/js.rs`,
// with its bindings in the directory given as the second argument.

import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { listenFramed, serveFramed } from "./transport.js";

const [addr, bindings] = process.argv.slice(2);
const echo = import(pathToFileURL(join(bindings, "echo-server/exports/wrpc-test/js/echo.js")).href);

listenFramed(addr, async (lis, sock) => {
  try {
    const { Asyncs } = await echo;
    for (let inv; (inv = await lis.accept());) {
      const [{ instance, name }, io] = inv;
      if (instance !== "wrpc-test:js/echo" || name !== "echo-async") {
        throw new Error(`\`${instance}#${name}\` is not served`);
      }
      await serveFramed(io, [Asyncs], [Asyncs], (v) => [v]);
    }
    await lis.io.tx.writer.close();
  } catch (err) {
    console.error("failed to serve:", err);
    sock.resetAndDestroy();
  }
});

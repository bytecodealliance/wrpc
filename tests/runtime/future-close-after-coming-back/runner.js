import { f } from "./a/b/the-test.js";

export async function run(connect) {
  const { promise, resolve } = Promise.withResolvers();
  await f(await connect(), promise);
  resolve();
}

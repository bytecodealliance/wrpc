import { f } from "./a/b/the-test.js";

export async function run(connect) {
  await f(await connect(), Promise.resolve());
}

import { f } from "./a/b/the-test.js";

export async function run(connect) {
  const { finished } = await f(await connect(), Promise.resolve());
  await finished;
}

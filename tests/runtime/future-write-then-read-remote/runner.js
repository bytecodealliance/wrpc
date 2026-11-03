import { f } from "./a/b/the-test.js";

export async function run(connect) {
  const { promise, resolve } = Promise.withResolvers();
  const [{ finished }] = await Promise.all([f(await connect(), promise), resolve()]);
  await finished;
}

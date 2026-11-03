import { get, set } from "./a/b/the-test.js";

export async function run(connect) {
  const { promise, resolve } = Promise.withResolvers();
  const [{ finished }] = await Promise.all([set(await connect(), promise), resolve()]);
  await finished;
  await get(await connect());
}

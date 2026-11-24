import { get, set } from "./a/b/the-test.js";

export async function run(connect) {
  const { promise, resolve } = Promise.withResolvers();
  await Promise.all([set(await connect(), promise), resolve()]);
  const [result, io] = await get(await connect());
  await Promise.all([result, io]);
}
